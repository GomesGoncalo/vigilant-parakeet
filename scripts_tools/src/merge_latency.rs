use anyhow::Context;
use serde_json::Value;
use std::fs::File;
use std::path::PathBuf;
use std::process::Command;

fn measure_avg(src: &str, dst: &str) -> Option<f64> {
    // determine if dst is a namespace; if so get its IPv4
    let mut is_ns = false;
    if let Ok(out) = Command::new("ip").args(["netns", "list"]).output() {
        let txt = String::from_utf8_lossy(&out.stdout).to_string();
        for line in txt.lines() {
            let token = line.split_whitespace().next().unwrap_or("");
            if token == dst {
                is_ns = true;
                break;
            }
        }
    }
    let mut dst_ip = dst.to_string();
    if is_ns {
        // dest is a namespace, pick first IPv4 from it
        let mut iptxt = String::new();
        if let Ok(out) = Command::new("ip")
            .args([
                "netns", "exec", dst, "ip", "-4", "addr", "show", "scope", "global",
            ])
            .output()
        {
            iptxt = String::from_utf8_lossy(&out.stdout).to_string();
        }
        if iptxt.trim().is_empty() {
            if let Ok(out) = Command::new("ip")
                .args(["netns", "exec", dst, "ip", "-4", "addr", "show"])
                .output()
            {
                iptxt = String::from_utf8_lossy(&out.stdout).to_string();
            }
        }
        for line in iptxt.lines() {
            if let Some(pos) = line.find("inet ") {
                let rest = &line[pos + 5..];
                let parts: Vec<&str> = rest.split_whitespace().collect();
                if !parts.is_empty() {
                    let a = parts[0].split('/').next().unwrap_or("");
                    if !a.is_empty() {
                        dst_ip = a.to_string();
                        break;
                    }
                }
            }
        }
    }

    let Ok(out) = Command::new("ip").args(["netns", "list"]).output() else {
        return None;
    };

    let txt = String::from_utf8_lossy(&out.stdout).to_string();
    let mut is_src_ns = false;
    for line in txt.lines() {
        if line.split_whitespace().next().unwrap_or("") == src {
            is_src_ns = true;
            break;
        }
    }
    if !is_src_ns {
        return None;
    }

    // prefer running the system ping inside the namespace (more reliable)
    let Some(v) = run_ping_netns(src, &dst_ip) else {
        eprintln!("probe in namespace {} failed for {}", src, dst_ip);
        return None;
    };

    Some(v)
}

/// Run the system `ping` inside a network namespace using `ip netns exec <ns> ping -c 1 -W 1 <addr>`
/// Returns latency in ms if parsed successfully.
fn run_ping_netns(ns: &str, addr: &str) -> Option<f64> {
    let out = Command::new("ip")
        .args(["netns", "exec", ns, "ping", "-c", "1", "-W", "1", addr])
        .output();
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !stderr.trim().is_empty() {
                eprintln!("[diag] ip netns exec ping stderr: {}", stderr.trim());
            }
            // look for 'time=XXX ms' in stdout
            if let Some(pos) = stdout.find("time=") {
                let rest = &stdout[pos + 5..];
                // rest starts with number like 0.123 ms
                let mut num_str = String::new();
                for c in rest.chars() {
                    if c.is_ascii_digit() || c == '.' {
                        num_str.push(c);
                    } else {
                        break;
                    }
                }
                if let Ok(v) = num_str.parse::<f64>() {
                    return Some(v);
                }
            }
            None
        }
        Err(e) => {
            eprintln!("[diag] failed to run ip netns exec ping: {}", e);
            None
        }
    }
}

pub fn run(ipf: PathBuf) -> anyhow::Result<()> {
    let f = File::open(&ipf).with_context(|| format!("opening {}", ipf.display()))?;
    let mut j: Value = serde_json::from_reader(f).context("parsing json")?;
    let runs = j
        .get("runs")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    use std::collections::HashMap;
    let mut latencies: HashMap<(String, String, String), Option<f64>> = HashMap::new();
    for r in &runs {
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
        let key = (src.clone(), dst.clone(), time.clone());
        if latencies.contains_key(&key) {
            continue;
        }
        println!("Measuring {} -> {}", src, dst);
        let avg = measure_avg(&src, &dst);
        latencies.insert(key, avg);
    }

    // attach to runs
    if let Some(runs_arr) = j.get_mut("runs").and_then(|v| v.as_array_mut()) {
        for r in runs_arr {
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
            let key = (src, dst, time);
            if let Some(Some(avg)) = latencies.get(&key) {
                // avg is already in ms from ping parse; keep 3 decimals
                if let Some(m) = r.as_object_mut() {
                    m.insert(
                        "latency_ms".to_string(),
                        serde_json::json!((avg * 1000.0).round() / 1000.0),
                    );
                }
            }
        }
    }

    // merge into summary
    if let Some(summary) = j.get_mut("summary").and_then(|v| v.as_array_mut()) {
        for s in summary {
            let src = s
                .get("src")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let dst = s
                .get("dst")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let time = s
                .get("time")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "0".to_string());
            let key = (src, dst, time);
            if let Some(Some(avg)) = latencies.get(&key) {
                if let Some(m) = s.as_object_mut() {
                    m.insert(
                        "latency_ms".to_string(),
                        serde_json::json!((avg * 1000.0).round() / 1000.0),
                    );
                }
            } else if let Some(m) = s.as_object_mut() {
                m.insert("latency_ms".to_string(), serde_json::Value::Null);
            }
        }
    }

    serde_json::to_writer_pretty(File::create(&ipf)?, &j)?;
    println!("Latency measurements merged into {}", ipf.display());
    Ok(())
}
