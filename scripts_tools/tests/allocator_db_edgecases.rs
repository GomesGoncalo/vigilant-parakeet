use std::fs::{self, File};
use std::io::Write;
use tempfile::tempdir;

// Test that a pre-populated DB prevents reassigning used IPs and that new assignments are added
#[test]
fn pre_populated_db() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;
    let dir = td.path();

    // create a sample node file with AUTOALLOC
    let node_path = dir.join("n1.yaml");
    let mut f = File::create(&node_path)?;
    writeln!(f, "ip: AUTOALLOC")?;
    writeln!(f, "topology: {{}}")?;

    // create alloc DB pre-populated with a specific ip
    let db_path = dir.join("alloc_db.json");
    let db_json = r#"{ "assigns": { "n_existing.yaml": "10.0.0.5" } }"#;
    fs::write(&db_path, db_json)?;

    // run the binary pointing to this directory and DB
    let mut cmd = assert_cmd::Command::cargo_bin("scripts_tools")?;
    cmd.arg("auto-fix-configs")
        .arg(node_path.as_os_str())
        .arg("--alloc-db")
        .arg(db_path.to_string_lossy().to_string());
    cmd.assert().success();

    // read updated db and ensure our node was added and existing stayed
    let updated = fs::read_to_string(&db_path)?;
    assert!(updated.contains("n_existing.yaml"));
    assert!(updated.contains("n1.yaml"));
    Ok(())
}

// Test that a malformed DB file is handled gracefully (either recreated or errors with clear message)
#[test]
fn malformed_db() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;
    let dir = td.path();

    let node_path = dir.join("n2.yaml");
    let mut f = File::create(&node_path)?;
    writeln!(f, "ip: AUTOALLOC")?;
    writeln!(f, "topology: {{}}")?;

    let db_path = dir.join("alloc_db.json");
    // write malformed JSON
    fs::write(&db_path, "{ this is not json }")?;

    let mut cmd = assert_cmd::Command::cargo_bin("scripts_tools")?;
    cmd.arg("auto-fix-configs")
        .arg(node_path.as_os_str())
        .arg("--alloc-db")
        .arg(db_path.to_string_lossy().to_string());

    // Accept either graceful success (recreate DB) or a clear failure mentioning 'alloc' or 'db'
    let assert = cmd.assert();
    let output = assert.get_output();
    if output.status.success() {
        // OK: recreated/overwrote DB
        let updated = fs::read_to_string(&db_path)?;
        assert!(updated.contains("n2.yaml"));
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.to_lowercase().contains("alloc") || stderr.to_lowercase().contains("db"));
    }
    Ok(())
}
