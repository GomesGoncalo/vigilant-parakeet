use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn autofix_dry_run_and_backup() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;
    let file = td.path().join("node1.yaml");
    fs::write(&file, "node_type: Obu\n")?;

    let mut cmd = Command::cargo_bin("scripts_tools")?;
    cmd.arg("autofixconfigs");
    cmd.arg("--dry-run");
    cmd.arg("--backup");
    cmd.arg(file.as_os_str());
    cmd.assert().success().stdout(predicate::str::contains("[dry-run]"));

    // no backup file should be created in dry-run
    let bak = file.with_extension("yaml.bak");
    assert!(!bak.exists());
    Ok(())
}

#[test]
fn autofix_apply_creates_backup() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;
    let file = td.path().join("node2.yaml");
    fs::write(&file, "node_type: Obu\n")?;

    let mut cmd = Command::cargo_bin("scripts_tools")?;
    cmd.arg("autofixconfigs");
    cmd.arg("--backup");
    cmd.arg(file.as_os_str());
    cmd.assert().success().stdout(predicate::str::contains("applied fixes"));

    let bak = file.with_extension("yaml.bak");
    assert!(bak.exists());
    Ok(())
}
