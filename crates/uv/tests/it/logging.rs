use anyhow::Result;
use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use crate::common::{uv_snapshot, TestContext};

/// `cache clean` should remove all packages.
#[test]
fn clean_all() -> Result<()> {
    let context = TestContext::new("3.12");

    // Write the requirements.txt so that something gets installed.
    let requirements_txt = context.temp_dir.child("requirements.txt");
    requirements_txt.write_str("typing-extensions\niniconfig")?;

    // Install a requirement, populating the cache.
    context
        .pip_sync()
        .arg("requirements.txt")
        .assert()
        .success();

    // Run the command under test (which writes to our log file).
    uv_snapshot!(context.with_filtered_counts().filters(),
        context.clean()
            .arg("--verbose"),@
r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    DEBUG uv [VERSION] ([COMMIT] DATE)
    Clearing cache at: [CACHE_DIR]/
    Removed [N] files ([SIZE])
    ");

    Ok(())
}