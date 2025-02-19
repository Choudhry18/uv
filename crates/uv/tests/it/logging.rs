use anyhow::Result;
use assert_cmd::prelude::*;
use assert_fs::prelude::*;
use std::fs;
use crate::common::{uv_snapshot, TestContext};

/// `cache clean` should remove all packages.
#[test]
fn clean_all() -> Result<()> {
    let context = TestContext::new("3.12");

    // Write the requirements.txt so that something gets installed.
    let path = std::env::current_dir()?;
    let requirements_txt = context.temp_dir.child("requirements.txt");
    requirements_txt.write_str("typing-extensions\niniconfig")?;

    // Install a requirement, populating the cache.
    context
        .pip_sync()
        .arg("requirements.txt")
        .assert()
        .success();

    // Determine log file name based on test function.
    let log_file = "clean_all";
    // Create a temporary file which we'll pin so it's not auto-deleted.
    let tmp_file = context.test_log.to_path_buf();

    // Print the temporary file path for debugging.
    println!("Temporary log file path: {:?}", tmp_file);

    // Run the command under test (which writes to our log file).
    uv_snapshot!(context.with_filtered_counts().filters(),
        context.clean()
            .arg("--verbose")
            .arg("--log")
            .arg("test"),@
r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    DEBUG uv [VERSION] ([COMMIT] DATE)
    Clearing cache at: [CACHE_DIR]/
    Removed [N] files ([SIZE])
    ");

    let dest_dir = path.join("tests/it/testLogs");
    // Ensure dest_dir exists (it already exists, but this is safe).
    fs::create_dir_all(&dest_dir).expect("failed to create destination directory");
    // Create a full destination file path.
    let destination = dest_dir.join(format!("{}.log", log_file));
    // Copy the persistent temporary file to the destination.
    fs::copy(tmp_file, &destination)
        .expect("failed to copy file");

    Ok(())
}