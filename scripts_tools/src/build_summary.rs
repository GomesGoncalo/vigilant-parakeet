use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
#[allow(dead_code)]
struct Row {
    src: String,
    dst: String,
    time: serde_json::Value,
    repeat: serde_json::Value,
    bandwidth_mbits: f64,
    raw_log: String,
}

#[derive(Serialize, Debug)]
struct SummaryEntry {
    src: String,
    dst: String,
    time: serde_json::Value,
    samples: usize,
    mean_mbits: f64,
    stddev_mbits: f64,
    min_mbits: f64,
    max_mbits: f64,
}

pub fn run(out_csv: PathBuf, json_out: PathBuf, summary_csv: PathBuf) -> anyhow::Result<()> {
    let mut rdr = csv::Reader::from_path(&out_csv)
        .with_context(|| format!("open csv {}", out_csv.display()))?;
    let headers = rdr.headers()?.clone();
    let mut rows: Vec<serde_json::Map<String, serde_json::Value>> = Vec::new();
    for result in rdr.records() {
        let record = result?;
        let mut map = serde_json::Map::new();
        for (i, h) in headers.iter().enumerate() {
            let val = record.get(i).unwrap_or("");
            match h {
                "bandwidth_mbits" => {
                    let parsed = val.parse::<f64>().unwrap_or(0.0);
                    map.insert("bandwidth_mbits".to_string(), serde_json::json!(parsed));
                }
                "time" => {
                    // try integer, else string
                    if let Ok(tv) = val.parse::<i64>() {
                        map.insert("time".to_string(), serde_json::json!(tv));
                    } else {
                        map.insert("time".to_string(), serde_json::json!(val));
                    }
                }
                _ => {
                    map.insert(h.to_string(), serde_json::json!(val));
                }
            }
        }
        // Ensure essential keys exist
        if !map.contains_key("src") {
            map.insert("src".to_string(), serde_json::json!(""));
        }
        if !map.contains_key("dst") {
            map.insert("dst".to_string(), serde_json::json!(""));
        }
        if !map.contains_key("time") {
            map.insert("time".to_string(), serde_json::json!(0));
        }
        rows.push(map);
    }

    eprintln!("build_summary: parsed {} rows", rows.len());
    if let Some(first) = rows.first() {
        eprintln!("build_summary: first row keys/values: {:?}", first);
    }

    use std::collections::HashMap;
    let mut groups: HashMap<(String, String, String), Vec<f64>> = HashMap::new();
    for r in &rows {
        let src = r
            .get("src")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let dst = r
            .get("dst")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let time = r
            .get("time")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "0".to_string());
        let bw = r
            .get("bandwidth_mbits")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        groups.entry((src, dst, time)).or_default().push(bw);
    }

    let mut summary: Vec<SummaryEntry> = Vec::new();
    for ((src, dst, time), vals) in groups {
        let samples = vals.len();
        let mean = if samples > 0 {
            vals.iter().sum::<f64>() / (samples as f64)
        } else {
            0.0
        };
        let stddev = if samples > 1 {
            let m = mean;
            let var = vals.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / ((samples - 1) as f64);
            var.sqrt()
        } else {
            0.0
        };
        let minv = vals.iter().cloned().fold(f64::INFINITY, f64::min);
        let maxv = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let minv = if minv.is_finite() { minv } else { 0.0 };
        let maxv = if maxv.is_finite() { maxv } else { 0.0 };
        summary.push(SummaryEntry {
            src,
            dst,
            time: serde_json::from_str(&time).unwrap_or(serde_json::json!(time)),
            samples,
            mean_mbits: (mean * 1000.0).round() / 1000.0,
            stddev_mbits: (stddev * 1000.0).round() / 1000.0,
            min_mbits: (minv * 1000.0).round() / 1000.0,
            max_mbits: (maxv * 1000.0).round() / 1000.0,
        });
    }

    // write json
    let out = serde_json::json!({"runs": rows, "summary": summary});
    serde_json::to_writer_pretty(File::create(&json_out)?, &out)?;

    // write summary csv
    let mut wtr = csv::Writer::from_path(&summary_csv)?;
    wtr.write_record([
        "src",
        "dst",
        "time",
        "samples",
        "mean_mbits",
        "stddev_mbits",
        "min_mbits",
        "max_mbits",
    ])?;
    for s in &summary {
        wtr.write_record([
            &s.src,
            &s.dst,
            &s.time.to_string(),
            &s.samples.to_string(),
            &format!("{:.3}", s.mean_mbits),
            &format!("{:.3}", s.stddev_mbits),
            &format!("{:.3}", s.min_mbits),
            &format!("{:.3}", s.max_mbits),
        ])?;
    }
    wtr.flush()?;

    println!("JSON results written to {}", json_out.display());
    println!("Summary CSV written to {}", summary_csv.display());
    Ok(())
}
