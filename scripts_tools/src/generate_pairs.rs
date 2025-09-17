use anyhow::Context;
use serde_json::Value;
use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

pub fn run(json: PathBuf, time: String, repeat: String) -> anyhow::Result<()> {
    let f = File::open(&json).with_context(|| format!("opening {}", json.display()))?;
    let data: Value = serde_json::from_reader(f).context("parsing json")?;
    let map = match data.as_object() {
        Some(m) => m,
        None => anyhow::bail!("JSON root is not an object"),
    };

    let mut obus = Vec::new();
    let mut rsus = Vec::new();
    for (name, info) in map.iter() {
        if let Some(nt) = info.get("node_type") {
            if nt == "Obu" {
                obus.push(name.clone());
            } else if nt == "Rsu" {
                rsus.push(name.clone());
            }
        }
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();
    for o in &obus {
        for r in &rsus {
            writeln!(out, "sim_ns_{},sim_ns_{},{},{}", o, r, time, repeat)?;
        }
    }
    Ok(())
}
