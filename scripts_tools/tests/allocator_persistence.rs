use assert_cmd::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn allocator_persists_assignments() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;
    let db = td.path().join("alloc.json");

    let f1 = td.path().join("a.yaml");
    fs::write(&f1, "node_type: Obu\n")?;

    // first run: assign one IP and persist DB
    let mut cmd = Command::cargo_bin("scripts_tools")?;
    cmd.arg("autofixconfigs");
    cmd.arg("--ip-cidr");
    cmd.arg("10.1.0.0/24");
    cmd.arg("--alloc-db");
    cmd.arg(db.as_os_str());
    cmd.arg(f1.as_os_str());
    cmd.assert().success();

    // read assigned ip
    let s1 = fs::read_to_string(&f1)?;
    let ip1 = s1
        .lines()
        .find(|l| l.trim_start().starts_with("ip:"))
        .unwrap()
        .split(':')
        .nth(1)
        .unwrap()
        .trim()
        .to_string();

    // create another file and run again with same DB
    let f2 = td.path().join("b.yaml");
    fs::write(&f2, "node_type: Obu\n")?;

    let mut cmd2 = Command::cargo_bin("scripts_tools")?;
    cmd2.arg("autofixconfigs");
    cmd2.arg("--ip-cidr");
    cmd2.arg("10.1.0.0/24");
    cmd2.arg("--alloc-db");
    cmd2.arg(db.as_os_str());
    cmd2.arg(f2.as_os_str());
    cmd2.assert().success();

    let s2 = fs::read_to_string(&f2)?;
    let ip2 = s2
        .lines()
        .find(|l| l.trim_start().starts_with("ip:"))
        .unwrap()
        .split(':')
        .nth(1)
        .unwrap()
        .trim()
        .to_string();

    assert_ne!(ip1, ip2);
    Ok(())
}
