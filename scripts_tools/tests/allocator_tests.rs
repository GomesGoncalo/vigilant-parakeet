use assert_cmd::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn allocator_assigns_unique_ips() -> Result<(), Box<dyn std::error::Error>> {
    let td = tempdir()?;

    // create three node files with AUTOALLOC
    let f1 = td.path().join("n1.yaml");
    let f2 = td.path().join("n2.yaml");
    let f3 = td.path().join("n3.yaml");

    fs::write(&f1, "node_type: Obu\n")?;
    fs::write(&f2, "node_type: Obu\n")?;
    // third already has an IP to simulate collision avoidance
    fs::write(&f3, "node_type: Obu\nip: 10.0.0.15\n")?;

    let mut cmd = Command::cargo_bin("scripts_tools")?;
    cmd.arg("autofixconfigs");
    cmd.arg("--ip-cidr");
    cmd.arg("10.0.0.0/24");
    cmd.arg("--backup");
    cmd.arg(f1.as_os_str());
    cmd.arg(f2.as_os_str());
    cmd.arg(f3.as_os_str());

    cmd.assert().success();

    // read resulting files
    let s1 = fs::read_to_string(&f1)?;
    let s2 = fs::read_to_string(&f2)?;
    let s3 = fs::read_to_string(&f3)?;

    // parse ips from YAML (simple substring checks)
    assert!(s1.contains("ip:"));
    assert!(s2.contains("ip:"));
    assert!(s3.contains("ip: 10.0.0.15"));

    // ensure assigned ips are unique
    let ip1 = s1
        .lines()
        .find(|l| l.trim_start().starts_with("ip:"))
        .unwrap()
        .split(':')
        .nth(1)
        .unwrap()
        .trim();
    let ip2 = s2
        .lines()
        .find(|l| l.trim_start().starts_with("ip:"))
        .unwrap()
        .split(':')
        .nth(1)
        .unwrap()
        .trim();
    let ip3 = s3
        .lines()
        .find(|l| l.trim_start().starts_with("ip:"))
        .unwrap()
        .split(':')
        .nth(1)
        .unwrap()
        .trim();

    assert_ne!(ip1, ip2);
    assert_ne!(ip1, ip3);
    assert_ne!(ip2, ip3);

    Ok(())
}
