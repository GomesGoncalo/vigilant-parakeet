use assert_cmd::Command;
use predicates::prelude::*;
use std::fs::{self, File};
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn parseband_prints_zero_on_empty() {
    let mut jf = NamedTempFile::new().unwrap();
    writeln!(jf, "{{}}",).unwrap();
    let mut cmd = Command::cargo_bin("scripts_tools").unwrap();
    cmd.arg("parseband").arg(jf.path());
    cmd.assert().success().stdout(predicate::str::contains("0"));
}

#[test]
fn buildsummary_creates_json_and_csv() {
    let mut csvf = NamedTempFile::new().unwrap();
    writeln!(csvf, "src,dst,time,repeat,bandwidth_mbits,raw_log").unwrap();
    writeln!(csvf, "a,b,5,1,10.5,/tmp/log").unwrap();
    let json_out = NamedTempFile::new().unwrap();
    let summary_out = NamedTempFile::new().unwrap();
    let mut cmd = Command::cargo_bin("scripts_tools").unwrap();
    cmd.arg("buildsummary")
        .arg(csvf.path())
        .arg(json_out.path())
        .arg(summary_out.path());
    cmd.assert().success();
    let j = fs::read_to_string(json_out.path()).unwrap();
    assert!(j.contains("runs"));
}

#[test]
fn nsaddrs_reads_stdin() {
    let mut cmd = Command::cargo_bin("scripts_tools").unwrap();
    cmd.arg("nsaddrs").write_stdin("10.0.0.1/24,10.0.0.2/24");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("10.0.0.1/24"));
}

#[test]
fn mergelatency_merges_latency_field() {
    // create a fake iperf results json
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    let content = r#"{ "runs": [ { "src": "s1", "dst": "d1", "time": 5 } ], "summary": [ { "src": "s1", "dst": "d1", "time": 5 } ] }"#;
    fs::write(&path, content).unwrap();

    // create a fake measure-latency.sh that outputs an rtt line
    fs::create_dir_all("scripts").ok();
    let mut sh = File::create("scripts/measure-latency.sh").unwrap();
    writeln!(sh, "#!/usr/bin/env bash").unwrap();
    writeln!(sh, "echo 'PING' >&2").unwrap();
    writeln!(
        sh,
        "echo 'rtt min/avg/max/mdev = 0.100/1.234/2.345/0.123 ms'"
    )
    .unwrap();
    drop(sh);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            "scripts/measure-latency.sh",
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }

    let mut cmd = Command::cargo_bin("scripts_tools").unwrap();
    cmd.arg("mergelatency").arg(&path);
    cmd.assert().success();
    let out = fs::read_to_string(&path).unwrap();
    assert!(out.contains("latency_ms"));

    // cleanup
    fs::remove_file("scripts/measure-latency.sh").ok();
}
