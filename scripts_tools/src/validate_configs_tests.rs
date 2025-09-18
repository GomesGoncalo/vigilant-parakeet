#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_tmp(name: &str, contents: &str) -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(name);
        let mut f = File::create(&p).unwrap();
        write!(f, "{}", contents).unwrap();
        // keep the temp dir to keep the file around for the test
        let _keep = dir.keep().unwrap();
        p
    }

    #[test]
    fn test_valid_node() {
        let p = write_tmp("node.yaml", "node_type: Obu\nip: 10.0.0.5\n");
        assert!(super::validate_configs::run(vec![p]).is_ok());
    }

    #[test]
    fn test_invalid_node_missing_ip() {
        let p = write_tmp("node2.yaml", "node_type: Obu\n");
        assert!(super::validate_configs::run(vec![p]).is_err());
    }

    #[test]
    fn test_valid_simulator() {
        let content = r#"nodes:\n  rsu1:\n    config_path: examples/n_rsu1.yaml\n topology:\n  rsu1:\n    rsu1:\n      latency: 0\n      loss: 0\n"#;
        let p = write_tmp("sim.yaml", content);
        assert!(super::validate_configs::run(vec![p]).is_ok());
    }
}
