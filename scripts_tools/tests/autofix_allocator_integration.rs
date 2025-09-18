use assert_cmd::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn autofix_and_allocator_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;

    // three node files: two AUTOALLOC, one preset
    let f1 = td.path().join("a.yaml");
    let f2 = td.path().join("b.yaml");
    let f3 = td.path().join("c.yaml");

    fs::write(&f1, "node_type: Obu\n")?;
    fs::write(&f2, "node_type: Obu\n")?;
    fs::write(&f3, "node_type: Obu\nip: 10.0.0.50\n")?;

    let dbpath = td.path().join("alloc_db.json");

    let mut cmd = Command::cargo_bin("scripts_tools")?;
    cmd.arg("autofixconfigs")
        .arg("--ip-cidr")
        .arg("10.0.0.0/24")
        .arg("--alloc-db")
        .arg(dbpath.as_os_str())
        .arg(f1.as_os_str())
        .arg(f2.as_os_str())
        .arg(f3.as_os_str());

    cmd.assert().success();

    // read files and db
    let s1 = fs::read_to_string(&f1)?;
    let s2 = fs::read_to_string(&f2)?;
    let s3 = fs::read_to_string(&f3)?;

    assert!(s1.contains("ip:"));
    assert!(s2.contains("ip:"));
    assert!(s3.contains("ip: 10.0.0.50"));

    let dbs = fs::read_to_string(&dbpath)?;
    assert!(dbs.contains("a.yaml") || dbs.contains("b.yaml"));

    Ok(())
}
