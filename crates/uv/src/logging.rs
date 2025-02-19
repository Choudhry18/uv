use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Context;
use jiff::Timestamp;
use owo_colors::OwoColorize;
use tracing::{Event, Subscriber};
#[cfg(feature = "tracing-durations-export")]
use tracing_durations_export::{
    plot::PlotConfig, DurationsLayer, DurationsLayerBuilder, DurationsLayerDropGuard,
};
use tracing_subscriber::filter::Directive;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};
use tracing_tree::time::Uptime;
use tracing_tree::HierarchicalLayer;

use uv_cli::ColorChoice;
#[cfg(feature = "tracing-durations-export")]
use uv_static::EnvVars;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    /// Suppress all tracing output by default (overridable by `RUST_LOG`).
    #[default]
    Default,
    /// Show debug messages by default (overridable by `RUST_LOG`).
    Verbose,
    /// Show messages in a hierarchical span tree. By default, debug messages are shown (overridable by `RUST_LOG`).
    ExtraVerbose,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogLevel {
    
    /// Write debug messages to the log file.
    #[default]
    Verbose,
    /// Write messages in a hierarchical span tree to the log file, debug messages are written.
    ExtraVerbose,
    /// Write trace level logs to the log file.
    TraceVerbose,
    /// Write messages in a hierarchical span tree to the log file, trace messages are written.
    TraceExtraVerbose,
}

struct UvFormat {
    display_timestamp: bool,
    display_level: bool,
    show_spans: bool,
}

/// See <https://docs.rs/tracing-subscriber/0.3.18/src/tracing_subscriber/fmt/format/mod.rs.html#1026-1156>
impl<S, N> FormatEvent<S, N> for UvFormat
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let ansi = writer.has_ansi_escapes();

        if self.display_timestamp {
            if ansi {
                write!(writer, "{} ", Timestamp::now().dimmed())?;
            } else {
                write!(writer, "{} ", Timestamp::now())?;
            }
        }

        if self.display_level {
            let level = meta.level();
            // Same colors as tracing
            if ansi {
                match *level {
                    tracing::Level::TRACE => write!(writer, "{} ", level.purple())?,
                    tracing::Level::DEBUG => write!(writer, "{} ", level.blue())?,
                    tracing::Level::INFO => write!(writer, "{} ", level.green())?,
                    tracing::Level::WARN => write!(writer, "{} ", level.yellow())?,
                    tracing::Level::ERROR => write!(writer, "{} ", level.red())?,
                }
            } else {
                write!(writer, "{level} ")?;
            }
        }

        if self.show_spans {
            let span = event.parent();
            let mut seen = false;

            let span = span
                .and_then(|id| ctx.span(id))
                .or_else(|| ctx.lookup_current());

            let scope = span.into_iter().flat_map(|span| span.scope().from_root());

            for span in scope {
                seen = true;
                if ansi {
                    write!(writer, "{}:", span.metadata().name().bold())?;
                } else {
                    write!(writer, "{}:", span.metadata().name())?;
                }
            }

            if seen {
                writer.write_char(' ')?;
            }
        }

        ctx.field_format().format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

/// Configure `tracing` based on the given [`Level`], taking into account the `RUST_LOG` environment
/// variable.
///
/// The [`Level`] is used to dictate the default filters (which can be overridden by the `RUST_LOG`
/// environment variable) along with the formatting of the output. For example, [`Level::Verbose`]
/// includes targets and timestamps, along with all `uv=debug` messages by default.
pub(crate) fn setup_logging(
    level: Level,
    durations: impl Layer<Registry> + Send + Sync,
    color: ColorChoice,
    log_path: &Option<PathBuf>,
    file_log_level: LogLevel,
) -> anyhow::Result<()> {
    let default_directive = match level {
        Level::Default => {
            // Show nothing, but allow `RUST_LOG` to override.
            tracing::level_filters::LevelFilter::OFF.into()
        }
        Level::Verbose | Level::ExtraVerbose => {
            // Show `DEBUG` messages from the CLI crate, but allow `RUST_LOG` to override.
            Directive::from_str("uv=debug").unwrap()
        }
    };

    // Only record our own spans.
    let durations_layer = durations.with_filter(
        tracing_subscriber::filter::Targets::new()
            .with_target("", tracing::level_filters::LevelFilter::INFO),
    );

    let filter = EnvFilter::builder()
        .with_default_directive(default_directive)
        .from_env()
        .context("Invalid RUST_LOG directives")?;

    let (ansi, color_choice) =
        match color.and_colorchoice(anstream::Stderr::choice(&std::io::stderr())) {
            ColorChoice::Always => (true, anstream::ColorChoice::Always),
            ColorChoice::Never => (false, anstream::ColorChoice::Never),
            ColorChoice::Auto => unreachable!("anstream can't return auto as choice"),
        };
    let writer = std::sync::Mutex::new(anstream::AutoStream::new(std::io::stderr(), color_choice));

    // Map file_log_level to a filter string.
    let file_filter_str = match file_log_level {
        LogLevel::Verbose|LogLevel::ExtraVerbose => "uv=debug",
        LogLevel::TraceVerbose| LogLevel::TraceExtraVerbose => "trace",
    };

    // Build the file filter from our mapping.
    let file_filter = EnvFilter::try_new(file_filter_str)
        .unwrap_or_else(|_| EnvFilter::new("uv=debug"));

    let subscriber = tracing_subscriber::registry()
        .with(durations_layer);

    let mut layers = Vec::new();


    // match the level to build the appropriate console layer and box it it as a type-erased traits object so that it can be added to the subscriber.
    match level {
        Level::Default | Level::Verbose => {
            // Regardless of the tracing level, show messages without any adornment.
            let format = UvFormat {
                display_timestamp: false,
                display_level: true,
                show_spans: false,
            };
            layers.push(tracing_subscriber::fmt::layer()
                .event_format(format)
                .with_writer(writer)
                .with_ansi(ansi)
                .with_filter(filter)
                .boxed());
        }
        Level::ExtraVerbose => {
            // Regardless of the tracing level, include the uptime and target for each message.
            layers.push(HierarchicalLayer::default()
                .with_targets(true)
                .with_timer(Uptime::default())
                .with_writer(writer)
                .with_ansi(ansi)
                .with_filter(filter)
                .boxed());
        }
    };


    if let Some(path) = log_path {
        let file_fomat = UvFormat {
            display_timestamp: false,
            display_level: true,
            show_spans: false,
        };
        let mut new_path = path.clone();
        new_path.set_extension("log");
        // Discuss if previous content should be overwritten or appended.
        // If it doesn't exist, create it. 
        let log_file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(&new_path)
        .with_context(|| format!("Failed to open or create log file: {:?}", new_path))?;
        
        let file_writer = std::sync::Mutex::new(anstream::AutoStream::new(log_file, anstream::ColorChoice::Never));

        match file_log_level {
            LogLevel::Verbose | LogLevel::TraceVerbose => {
                layers.push(tracing_subscriber::fmt::layer()
                    .event_format(file_fomat)
                    .with_writer(file_writer)
                    .with_ansi(false)
                    .with_filter(file_filter).boxed());
            }
            LogLevel::ExtraVerbose | LogLevel::TraceExtraVerbose=> {
                layers.push(
                HierarchicalLayer::default()
                    .with_writer(file_writer)
                    .with_ansi(false)
                    .with_filter(file_filter).boxed());
            }
        }
    } 
    subscriber.with(layers).init();

    Ok(())
}

/// Setup the `TRACING_DURATIONS_FILE` environment variable to enable tracing durations.
#[cfg(feature = "tracing-durations-export")]
pub(crate) fn setup_duration() -> anyhow::Result<(
    Option<DurationsLayer<Registry>>,
    Option<DurationsLayerDropGuard>,
)> {
    if let Ok(location) = std::env::var(EnvVars::TRACING_DURATIONS_FILE) {
        let location = std::path::PathBuf::from(location);
        if let Some(parent) = location.parent() {
            fs_err::create_dir_all(parent)
                .context("Failed to create parent of TRACING_DURATIONS_FILE")?;
        }
        let plot_config = PlotConfig {
            multi_lane: true,
            min_length: None,
            remove: Some(
                ["get_cached_with_callback".to_string()]
                    .into_iter()
                    .collect(),
            ),
            ..PlotConfig::default()
        };
        let (layer, guard) = DurationsLayerBuilder::default()
            .durations_file(&location)
            .plot_file(location.with_extension("svg"))
            .plot_config(plot_config)
            .build()
            .context("Couldn't create TRACING_DURATIONS_FILE files")?;
        Ok((Some(layer), Some(guard)))
    } else {
        Ok((None, None))
    }
}
