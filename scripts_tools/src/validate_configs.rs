use anyhow::Context;
use serde_yaml::Value as YamlValue;
use std::fs::File;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

fn expect_mapping<'a>(v: &'a YamlValue, path: &Path) -> anyhow::Result<&'a serde_yaml::Mapping> {
    match v.as_mapping() {
        Some(m) => Ok(m),
        None => anyhow::bail!("{}: expected top-level YAML mapping", path.display()),
    }
}

fn check_node_yaml(map: &serde_yaml::Mapping, path: &Path) -> anyhow::Result<()> {
    // node_type required and must be Obu or Rsu
    let node_type = map
        .get(YamlValue::from("node_type"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let node_type = node_type
        .ok_or_else(|| anyhow::anyhow!("{}: missing required field 'node_type'", path.display()))?;
    if node_type != "Obu" && node_type != "Rsu" {
        anyhow::bail!(
            "{}: node_type must be 'Obu' or 'Rsu', got '{}'",
            path.display(),
            node_type
        );
    }

    // ip required and must be a valid IPv4 or IPv6 address
    let ip_val = map
        .get(YamlValue::from("ip"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("{}: missing required field 'ip'", path.display()))?;
    if ip_val.parse::<IpAddr>().is_err() {
        anyhow::bail!(
            "{}: ip field is not a valid IP address: {}",
            path.display(),
            ip_val
        );
    }

    Ok(())
}

fn check_simulator_yaml(map: &serde_yaml::Mapping, path: &Path) -> anyhow::Result<()> {
    // nodes: mapping of name -> {config_path: string}
    let nodes_val = map
        .get(YamlValue::from("nodes"))
        .ok_or_else(|| anyhow::anyhow!("{}: missing top-level 'nodes' mapping", path.display()))?;
    let nodes_map = nodes_val
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("{}: 'nodes' should be a mapping", path.display()))?;
    if nodes_map.is_empty() {
        anyhow::bail!("{}: 'nodes' mapping is empty", path.display());
    }
    for (k, v) in nodes_map.iter() {
        let name = k.as_str().unwrap_or("<non-string key>");
        let node_cfg = v.as_mapping().ok_or_else(|| {
            anyhow::anyhow!(
                "{}: node '{}' entry should be a mapping",
                path.display(),
                name
            )
        })?;
        if !node_cfg.contains_key(YamlValue::from("config_path")) {
            anyhow::bail!("{}: node '{}' missing 'config_path'", path.display(), name);
        }
    }

    // topology: mapping of node -> peer -> {latency: num, loss: num}
    let topo_val = map.get(YamlValue::from("topology")).ok_or_else(|| {
        anyhow::anyhow!("{}: missing top-level 'topology' mapping", path.display())
    })?;
    let topo_map = topo_val
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("{}: 'topology' should be a mapping", path.display()))?;
    if topo_map.is_empty() {
        anyhow::bail!("{}: 'topology' mapping is empty", path.display());
    }
    for (node_k, peers_v) in topo_map.iter() {
        let node_name = node_k.as_str().unwrap_or("<non-string key>");
        let peers = peers_v.as_mapping().ok_or_else(|| {
            anyhow::anyhow!(
                "{}: topology entry for '{}' should be a mapping",
                path.display(),
                node_name
            )
        })?;
        for (peer_k, metrics_v) in peers.iter() {
            let peer_name = peer_k.as_str().unwrap_or("<non-string key>");
            let metrics = metrics_v.as_mapping().ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: topology '{}->{}' should have mapping with latency/loss",
                    path.display(),
                    node_name,
                    peer_name
                )
            })?;
            // check latency and loss exist and are numbers
            let latency = metrics
                .get(YamlValue::from("latency"))
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)));
            let loss = metrics
                .get(YamlValue::from("loss"))
                .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)));
            if latency.is_none() || loss.is_none() {
                anyhow::bail!(
                    "{}: topology '{}->{}' missing numeric 'latency' or 'loss'",
                    path.display(),
                    node_name,
                    peer_name
                );
            }
        }
    }

    Ok(())
}

fn check_yaml(path: &Path) -> anyhow::Result<()> {
    let mut s = String::new();
    File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_to_string(&mut s)
        .with_context(|| format!("reading {}", path.display()))?;
    let v: YamlValue =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    let map = expect_mapping(&v, path)?;

    // Distinguish simulator vs node based on presence of top-level keys
    if map.contains_key(YamlValue::from("nodes")) || map.contains_key(YamlValue::from("topology")) {
        check_simulator_yaml(map, path)?;
    } else if map.contains_key(YamlValue::from("node_type")) {
        check_node_yaml(map, path)?;
    } else {
        anyhow::bail!("{}: unknown YAML type (neither simulator nor node); expected 'nodes'/'topology' or 'node_type'", path.display());
    }
    Ok(())
}

pub fn run(paths: Vec<PathBuf>) -> anyhow::Result<()> {
    if paths.is_empty() {
        anyhow::bail!("No files provided to validate");
    }
    let mut failed = false;
    for p in paths.iter() {
        match check_yaml(p) {
            Ok(()) => println!("OK: {}", p.display()),
            Err(e) => {
                eprintln!("ERROR: {} -> {}", p.display(), e);
                failed = true;
            }
        }
    }
    if failed {
        anyhow::bail!("One or more files failed validation");
    }
    Ok(())
}
