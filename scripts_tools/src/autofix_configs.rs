use anyhow::Context;
use serde_yaml::Value as YamlValue;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

type FileYamlPair = (PathBuf, YamlValue);
type CollectResult = (Vec<FileYamlPair>, HashSet<std::net::Ipv4Addr>);

fn read_yaml(path: &PathBuf) -> anyhow::Result<YamlValue> {
    let mut s = String::new();
    File::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .read_to_string(&mut s)
        .with_context(|| format!("reading {}", path.display()))?;
    let v: YamlValue =
        serde_yaml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
    Ok(v)
}

fn write_yaml(path: &PathBuf, v: &YamlValue) -> anyhow::Result<()> {
    let s = serde_yaml::to_string(v)?;
    let mut f = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    f.write_all(s.as_bytes())?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct AllocOptions {
    pub cidr: Option<String>,
    pub default_latency: i64,
    pub default_loss: i64,
    pub dry_run: bool,
    pub backup: bool,
    pub alloc_db: Option<String>,
    pub start_offset: u8,
}

pub fn run(paths: Vec<PathBuf>, dry_run: bool, backup: bool) -> anyhow::Result<()> {
    if paths.is_empty() {
        anyhow::bail!("no files provided to autofix");
    }
    for p in paths.iter() {
        let mut v = read_yaml(p)?;
        let mut changed = false;
        if let Some(map) = v.as_mapping_mut() {
            if map.contains_key(YamlValue::from("node_type")) {
                // node file: ensure ip exists
                if !map.contains_key(YamlValue::from("ip")) {
                    // assign a placeholder IP later via allocator marker
                    map.insert(YamlValue::from("ip"), YamlValue::from("AUTOALLOC"));
                    changed = true;
                    println!("will set ip=AUTOALLOC for {}", p.display());
                } else {
                    // validate ip
                    if let Some(ip_s) = map.get(YamlValue::from("ip")).and_then(|v| v.as_str()) {
                        if ip_s.parse::<IpAddr>().is_err() {
                            println!(
                                "invalid ip '{}' in {}, replacing with AUTOALLOC",
                                ip_s,
                                p.display()
                            );
                            map.insert(YamlValue::from("ip"), YamlValue::from("AUTOALLOC"));
                            changed = true;
                        }
                    }
                }
            } else if map.contains_key(YamlValue::from("nodes"))
                || map.contains_key(YamlValue::from("topology"))
            {
                // simulator: ensure nodes have config_path
                if let Some(nodes) = map
                    .get_mut(YamlValue::from("nodes"))
                    .and_then(|n| n.as_mapping_mut())
                {
                    for (k, v) in nodes.iter_mut() {
                        if let Some(node_map) = v.as_mapping_mut() {
                            if !node_map.contains_key(YamlValue::from("config_path")) {
                                // default to examples/<name>.yaml
                                if let Some(name) = k.as_str() {
                                    let default = format!("examples/{name}.yaml");
                                    node_map.insert(
                                        YamlValue::from("config_path"),
                                        YamlValue::from(default),
                                    );
                                    changed = true;
                                    println!(
                                        "will set config_path for {name} to examples/{name}.yaml"
                                    );
                                }
                            }
                        }
                    }
                }
                // populate missing latency/loss with defaults
                if let Some(topo) = map
                    .get_mut(YamlValue::from("topology"))
                    .and_then(|t| t.as_mapping_mut())
                {
                    for (_node, peers_v) in topo.iter_mut() {
                        if let Some(peers) = peers_v.as_mapping_mut() {
                            for (_peer, metrics_v) in peers.iter_mut() {
                                if let Some(metrics) = metrics_v.as_mapping_mut() {
                                    if !metrics.contains_key(YamlValue::from("latency")) {
                                        metrics.insert(
                                            YamlValue::from("latency"),
                                            YamlValue::from(10),
                                        );
                                        changed = true;
                                    }
                                    if !metrics.contains_key(YamlValue::from("loss")) {
                                        metrics.insert(YamlValue::from("loss"), YamlValue::from(0));
                                        changed = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if changed {
            if dry_run {
                println!("[dry-run] would modify {}", p.display());
            } else {
                if backup {
                    let bak = p.with_extension("yaml.bak");
                    fs::copy(p, &bak).with_context(|| {
                        format!("backing up {} to {}", p.display(), bak.display())
                    })?;
                }
                write_yaml(p, &v)?;
                println!("applied fixes to {}", p.display());
            }
        } else {
            println!("no changes for {}", p.display());
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct AllocDB {
    assigns: BTreeMap<String, String>,
}

pub fn apply_allocator(paths: Vec<PathBuf>, opts: AllocOptions) -> anyhow::Result<()> {
    if paths.is_empty() {
        anyhow::bail!("no files provided to allocator");
    }
    // helper-driven setup
    let base_net = parse_base_net(&opts)?;
    let mut db = load_alloc_db(opts.alloc_db.as_ref())?;
    let (files, mut used) = collect_used_ips(&paths, &db)?;
    let mut alloc_iter = build_candidates(base_net, opts.start_offset).into_iter();

    for (p, mut v) in files.into_iter() {
        if let Some(map) = v.as_mapping_mut() {
            let mut changed = false;
            // assignment and topology patching extracted to helper for clarity
            if assign_ip_for_file(&p, map, &mut alloc_iter, &mut used, &mut db, &opts)? {
                changed = true;
            }

            if let Some(topo) = map
                .get_mut(YamlValue::from("topology"))
                .and_then(|t| t.as_mapping_mut())
            {
                for (_node, peers_v) in topo.iter_mut() {
                    if let Some(peers) = peers_v.as_mapping_mut() {
                        for (_peer, metrics_v) in peers.iter_mut() {
                            if let Some(metrics) = metrics_v.as_mapping_mut() {
                                if !metrics.contains_key(YamlValue::from("latency")) {
                                    metrics.insert(
                                        YamlValue::from("latency"),
                                        YamlValue::from(opts.default_latency),
                                    );
                                    changed = true;
                                }
                                if !metrics.contains_key(YamlValue::from("loss")) {
                                    metrics.insert(
                                        YamlValue::from("loss"),
                                        YamlValue::from(opts.default_loss),
                                    );
                                    changed = true;
                                }
                            }
                        }
                    }
                }
            }

            if changed {
                if opts.dry_run {
                    println!("[dry-run] would write allocated fixes to {}", p.display());
                } else {
                    if opts.backup {
                        let bak = p.with_extension("yaml.bak");
                        fs::copy(&p, &bak)?;
                    }
                    let s = serde_yaml::to_string(&v)?;
                    let mut f = File::create(&p)?;
                    f.write_all(s.as_bytes())?;
                    println!("wrote fixes to {}", p.display());
                }
            } else {
                println!("no allocator changes for {}", p.display());
            }
        }
    }

    if let Some(dbpath) = opts.alloc_db.as_ref() {
        let s = serde_json::to_string_pretty(&db)?;
        if !opts.dry_run {
            std::fs::write(dbpath, s)?;
        } else {
            println!("[dry-run] would write alloc db to {dbpath}");
        }
    }

    Ok(())
}

fn parse_base_net(opts: &AllocOptions) -> anyhow::Result<ipnetwork::Ipv4Network> {
    if let Some(c) = opts.cidr.as_ref() {
        match c.parse::<IpNetwork>() {
            Ok(IpNetwork::V4(n)) => Ok(n),
            Ok(IpNetwork::V6(_)) => anyhow::bail!("ipv6 cidr not supported for allocator"),
            Err(e) => anyhow::bail!("invalid cidr '{}': {}", c, e),
        }
    } else {
        Ok(ipnetwork::Ipv4Network::new("10.0.0.0".parse()?, 24)?)
    }
}

fn load_alloc_db(dbpath: Option<&String>) -> anyhow::Result<AllocDB> {
    let mut db = AllocDB {
        assigns: BTreeMap::new(),
    };
    if let Some(path) = dbpath.as_ref() {
        if std::path::Path::new(path).exists() {
            let s = std::fs::read_to_string(path)?;
            if let Ok(parsed) = serde_json::from_str::<AllocDB>(&s) {
                db = parsed;
            }
        }
    }
    Ok(db)
}

fn collect_used_ips(paths: &[PathBuf], db: &AllocDB) -> anyhow::Result<CollectResult> {
    let mut used = HashSet::<std::net::Ipv4Addr>::new();
    let mut files: Vec<FileYamlPair> = Vec::new();
    for p in paths.iter() {
        let mut s = String::new();
        File::open(p)?.read_to_string(&mut s)?;
        let v: YamlValue = serde_yaml::from_str(&s)?;
        if let Some(map) = v.as_mapping() {
            if let Some(ipv) = map.get(YamlValue::from("ip")).and_then(|v| v.as_str()) {
                if ipv != "AUTOALLOC" {
                    if let Ok(ip) = ipv.parse::<std::net::Ipv4Addr>() {
                        used.insert(ip);
                    }
                }
            }
        }
        files.push((p.clone(), v));
    }

    for (_k, v) in db.assigns.iter() {
        if let Ok(ip) = v.parse::<std::net::Ipv4Addr>() {
            used.insert(ip);
        }
    }

    Ok((files, used))
}

pub fn build_candidates(
    base_net: ipnetwork::Ipv4Network,
    start_offset: u8,
) -> Vec<std::net::Ipv4Addr> {
    let net_addr = base_net.network();
    let bcast_addr = base_net.broadcast();
    let net_u32 = u32::from(net_addr);
    let bcast_u32 = u32::from(bcast_addr);
    let mut candidates: Vec<std::net::Ipv4Addr> = Vec::new();
    if bcast_u32 > net_u32 + 1 {
        for ip_u in (net_u32 + 1)..(bcast_u32) {
            candidates.push(std::net::Ipv4Addr::from(ip_u));
        }
    }
    let start_idx = candidates
        .iter()
        .position(|ip| ip.octets()[3] >= start_offset)
        .unwrap_or(0);
    candidates.into_iter().skip(start_idx).collect()
}

fn assign_ip_for_file(
    p: &Path,
    map: &mut serde_yaml::Mapping,
    alloc_iter: &mut impl Iterator<Item = std::net::Ipv4Addr>,
    used: &mut HashSet<std::net::Ipv4Addr>,
    db: &mut AllocDB,
    opts: &AllocOptions,
) -> anyhow::Result<bool> {
    // try to assign ip, then patch topology defaults
    let mut changed = false;
    if try_assign_ip(p, map, alloc_iter, used, db)? {
        changed = true;
    }
    if patch_topology_defaults(map, opts)? {
        changed = true;
    }
    Ok(changed)
}

fn try_assign_ip(
    p: &Path,
    map: &mut serde_yaml::Mapping,
    alloc_iter: &mut impl Iterator<Item = std::net::Ipv4Addr>,
    used: &mut HashSet<std::net::Ipv4Addr>,
    db: &mut AllocDB,
) -> anyhow::Result<bool> {
    if let Some(ip_val) = map.get(YamlValue::from("ip")).and_then(|v| v.as_str()) {
        if ip_val == "AUTOALLOC" {
            for candidate in alloc_iter.by_ref() {
                if !used.contains(&candidate) {
                    used.insert(candidate);
                    let a_s = candidate.to_string();
                    db.assigns.insert(p.display().to_string(), a_s.clone());
                    map.insert(YamlValue::from("ip"), YamlValue::from(a_s.clone()));
                    println!("assigned ip {} for {}", candidate, p.display());
                    return Ok(true);
                }
            }
            println!("no available ip to assign for {}", p.display());
        }
    }
    Ok(false)
}

fn patch_topology_defaults(
    map: &mut serde_yaml::Mapping,
    opts: &AllocOptions,
) -> anyhow::Result<bool> {
    let mut changed = false;
    if let Some(topo) = map
        .get_mut(YamlValue::from("topology"))
        .and_then(|t| t.as_mapping_mut())
    {
        for (_node, peers_v) in topo.iter_mut() {
            if let Some(peers) = peers_v.as_mapping_mut() {
                for (_peer, metrics_v) in peers.iter_mut() {
                    if let Some(metrics) = metrics_v.as_mapping_mut() {
                        if !metrics.contains_key(YamlValue::from("latency")) {
                            metrics.insert(
                                YamlValue::from("latency"),
                                YamlValue::from(opts.default_latency),
                            );
                            changed = true;
                        }
                        if !metrics.contains_key(YamlValue::from("loss")) {
                            metrics.insert(
                                YamlValue::from("loss"),
                                YamlValue::from(opts.default_loss),
                            );
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_options_default_cidr() {
        let opts = AllocOptions {
            cidr: None,
            default_latency: 5,
            default_loss: 0,
            dry_run: true,
            backup: false,
            alloc_db: None,
            start_offset: 10,
        };
        assert!(opts.cidr.is_none());
        assert_eq!(opts.start_offset, 10);
        let net = parse_base_net(&opts).expect("parse base net");
        assert_eq!(net.prefix(), 24);
    }

    #[test]
    fn alloc_options_with_cidr_string() {
        let opts = AllocOptions {
            cidr: Some("10.1.2.0/24".to_string()),
            default_latency: 5,
            default_loss: 0,
            dry_run: true,
            backup: false,
            alloc_db: None,
            start_offset: 5,
        };
        assert_eq!(opts.cidr.as_deref(), Some("10.1.2.0/24"));
        assert_eq!(opts.start_offset, 5);
        let net = parse_base_net(&opts).expect("parse base net");
        assert_eq!(net.network().octets()[0], 10);
    }

    #[test]
    fn build_candidates_respects_start_offset() {
        let net = ipnetwork::Ipv4Network::new("192.168.0.0".parse().unwrap(), 29).unwrap();
        let cands = build_candidates(net, 3);
        // /29 has hosts .1...6, start_offset 3 should skip until last octet >=3
        assert!(cands.iter().all(|ip| ip.octets()[3] >= 3));
    }

    #[test]
    fn try_assign_ip_assigns_and_updates_db() {
        use std::str::FromStr;
        // prepare a mapping with AUTOALLOC
        let mut map = serde_yaml::Mapping::new();
        map.insert(YamlValue::from("ip"), YamlValue::from("AUTOALLOC"));

        let mut used = HashSet::new();
        // create alloc iterator starting at 10.0.0.10
        let start = std::net::Ipv4Addr::from_str("10.0.0.10").unwrap();
        let mut alloc_iter = vec![start].into_iter();
        let mut db = AllocDB {
            assigns: BTreeMap::new(),
        };
        let p = PathBuf::from("node.yaml");

        let assigned =
            try_assign_ip(&p, &mut map, &mut alloc_iter, &mut used, &mut db).expect("assign ok");
        assert!(assigned);
        // ip field replaced
        let ip_field = map
            .get(YamlValue::from("ip"))
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(ip_field, "10.0.0.10");
        // db contains assignment
        assert!(db.assigns.contains_key(&p.display().to_string()));
    }

    #[test]
    fn patch_topology_defaults_inserts_latency_and_loss() {
        // topology: { n1: { n2: {} } }
        let mut map = serde_yaml::Mapping::new();
        let mut topo = serde_yaml::Mapping::new();
        let mut peers = serde_yaml::Mapping::new();
        peers.insert(
            YamlValue::from("n2"),
            YamlValue::from(serde_yaml::Mapping::new()),
        );
        topo.insert(YamlValue::from("n1"), YamlValue::from(peers));
        map.insert(YamlValue::from("topology"), YamlValue::from(topo));

        let opts = AllocOptions {
            cidr: None,
            default_latency: 77,
            default_loss: 3,
            dry_run: true,
            backup: false,
            alloc_db: None,
            start_offset: 10,
        };

        let changed = patch_topology_defaults(&mut map, &opts).expect("patch ok");
        assert!(changed);
        // verify inserted values
        if let Some(t) = map
            .get(YamlValue::from("topology"))
            .and_then(|v| v.as_mapping())
        {
            let n1 = t
                .get(YamlValue::from("n1"))
                .and_then(|v| v.as_mapping())
                .unwrap();
            let n2 = n1
                .get(YamlValue::from("n2"))
                .and_then(|v| v.as_mapping())
                .unwrap();
            assert_eq!(
                n2.get(YamlValue::from("latency")).and_then(|v| v.as_i64()),
                Some(77)
            );
            assert_eq!(
                n2.get(YamlValue::from("loss")).and_then(|v| v.as_i64()),
                Some(3)
            );
        } else {
            panic!("topology missing");
        }
    }
}
