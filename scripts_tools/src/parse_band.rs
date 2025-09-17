use anyhow::Context;
use serde_json::Value;
use std::fs::File;
use std::path::PathBuf;

pub fn run(json: PathBuf) -> anyhow::Result<()> {
    let f = File::open(&json).with_context(|| format!("opening {}", json.display()))?;
    let j: Value = serde_json::from_reader(f).context("parsing json")?;
    let mut b: Option<f64> = None;
    if let Some(end) = j.get("end") {
        if let Some(sum_received) = end.get("sum_received") {
            if let Some(v) = sum_received.get("bits_per_second") {
                b = v.as_f64();
            }
        }
        if b.is_none() {
            if let Some(sum_sent) = end.get("sum_sent") {
                if let Some(v) = sum_sent.get("bits_per_second") {
                    b = v.as_f64();
                }
            }
        }
    }
    if b.is_none() {
        if let Some(intervals) = j.get("intervals") {
            if let Some(last) = intervals.as_array().and_then(|a| a.last()) {
                if let Some(sum) = last.get("sum") {
                    if let Some(v) = sum.get("bits_per_second") {
                        b = v.as_f64();
                    }
                }
            }
        }
    }
    let out = match b {
        Some(x) => format!("{:.3}", x / 1e6),
        None => "0".to_string(),
    };
    println!("{}", out);
    Ok(())
}
