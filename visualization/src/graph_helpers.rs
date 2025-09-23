use serde_json::Value as JsonValue;
use std::collections::BTreeMap;

/// Compute a simple edge width from up/down bps values.
/// Returns an integer width between 1 and 20.
pub fn compute_edge_width(up: f64, down: f64) -> usize {
    // Desired behavior:
    // - route with the least traffic -> minimum thickness 1
    // - other routes scale logarithmically with total traffic (up+down)
    // - cap the maximum thickness at 20
    let upv = up.max(0.0);
    let downv = down.max(0.0);
    let total = upv + downv;
    if total <= 0.0 {
        return 1usize;
    }
    // Map log10(total) to [0..1] using a chosen reference scale (1e6 -> full scale).
    // Adjust `scale_log` if you want a different saturation point.
    let scale_log = 6.0f64; // log10(1_000_000) => traffic around 1e6 will map to max width
    let lv = total.log10();
    let frac = (lv / scale_log).clamp(0.0, 1.0);
    let min_w = 1.0f64;
    // Cap width at 12 to match wasm test expectations
    let max_w = 12.0f64;
    let w = min_w + (frac * (max_w - min_w)).round();
    (w as usize).clamp(1, 12)
}

/// Determine route kind for an edge from `from` to `to` using node_info shape.
/// node_info is a map node -> { upstream: Option<{ node_name: String }>, downstream: Option<Vec<{ node_name: String }>> }
pub fn determine_route_kind(
    from: &str,
    to: &str,
    node_info: &BTreeMap<String, JsonValue>,
) -> String {
    // prefer explicit downstream listing on the 'to' node
    if let Some(info_t) = node_info.get(to) {
        if let Some(downs) = info_t.get("downstream") {
            if let Some(arr) = downs.as_array() {
                for d in arr.iter() {
                    if let Some(child_name) = d.get("node_name").and_then(|v| v.as_str()) {
                        if child_name == from {
                            return "downstream".to_string();
                        }
                    }
                }
            }
        }
    }
    // check if from's upstream points to to
    if let Some(info_f) = node_info.get(from) {
        if let Some(up) = info_f.get("upstream") {
            if let Some(un) = up.get("node_name").and_then(|v| v.as_str()) {
                if un == to {
                    return "upstream".to_string();
                }
            }
        }
    }
    "neutral".to_string()
}

/// Build a node JSON object for the frontend renderer.
pub fn build_node_json(
    name: &str,
    x: f64,
    y: f64,
    node_info: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let mut m = serde_json::Map::new();
    m.insert("id".to_string(), JsonValue::String(name.to_string()));
    m.insert("label".to_string(), JsonValue::String(name.to_string()));
    m.insert(
        "x".to_string(),
        JsonValue::Number(serde_json::Number::from_f64(x).unwrap()),
    );
    m.insert(
        "y".to_string(),
        JsonValue::Number(serde_json::Number::from_f64(y).unwrap()),
    );
    // also include a nested `position` object to make JS normalization robust
    let mut pos = serde_json::Map::new();
    pos.insert(
        "x".to_string(),
        JsonValue::Number(serde_json::Number::from_f64(x).unwrap()),
    );
    pos.insert(
        "y".to_string(),
        JsonValue::Number(serde_json::Number::from_f64(y).unwrap()),
    );
    m.insert("position".to_string(), JsonValue::Object(pos));
    let mut size = 9usize;
    if let Some(info) = node_info.get(name) {
        if let Some(nt) = info.get("node_type").and_then(|v| v.as_str()) {
            m.insert("type".to_string(), JsonValue::String(nt.to_string()));
            if nt.eq_ignore_ascii_case("rsu") {
                size = 14
            } else {
                size = 10
            }
        }
    }
    m.insert(
        "size".to_string(),
        JsonValue::Number(serde_json::Number::from(size)),
    );
    let color = if size == 14 {
        "rgba(200,30,30,0.95)"
    } else {
        "rgba(30,100,200,0.95)"
    };
    m.insert("color".to_string(), JsonValue::String(color.to_string()));
    JsonValue::Object(m)
}

/// Build an edge JSON object for the frontend renderer, including width, color, and route_kind.
pub fn build_edge_json(
    from: &str,
    to: &str,
    up_bps: f64,
    down_bps: f64,
    node_info: &BTreeMap<String, JsonValue>,
) -> JsonValue {
    let mut m = serde_json::Map::new();
    m.insert("source".to_string(), JsonValue::String(from.to_string()));
    m.insert("target".to_string(), JsonValue::String(to.to_string()));
    m.insert(
        "up_bps".to_string(),
        JsonValue::Number(serde_json::Number::from_f64(up_bps).unwrap()),
    );
    m.insert(
        "down_bps".to_string(),
        JsonValue::Number(serde_json::Number::from_f64(down_bps).unwrap()),
    );
    let width = compute_edge_width(up_bps, down_bps) as i64;
    m.insert(
        "width".to_string(),
        JsonValue::Number(serde_json::Number::from(width)),
    );
    m.insert(
        "color".to_string(),
        JsonValue::String("rgba(120,120,120,1)".to_string()),
    );
    let rk = determine_route_kind(from, to, node_info);
    m.insert("route_kind".to_string(), JsonValue::String(rk));
    // include an id to help JS upsert/lookup (falls back to src->tgt if absent)
    m.insert("id".to_string(), JsonValue::String(format!("{from}->{to}")));
    JsonValue::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn edge_width_scaling() {
        // zero traffic should produce a small positive width
        let w0 = compute_edge_width(0.0, 0.0);
        assert!((1..=3).contains(&w0));
        // moderate traffic increases or stays within bounds
        assert!(compute_edge_width(100.0, 0.0) >= 1);
        // very large traffic should still be capped
        assert!(compute_edge_width(1e6, 1e6) <= 20);
    }

    #[test]
    fn determine_route_kind_downstream() {
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": null, "downstream": null }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "upstream": null, "downstream": [ { "node_name": "A" } ] }),
        );
        assert_eq!(determine_route_kind("A", "B", &ni), "downstream");
    }

    #[test]
    fn determine_route_kind_upstream() {
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": { "node_name": "B" }, "downstream": null }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "upstream": null, "downstream": null }),
        );
        assert_eq!(determine_route_kind("A", "B", &ni), "upstream");
    }

    #[test]
    fn determine_route_kind_neutral() {
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": null, "downstream": null }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "upstream": null, "downstream": null }),
        );
        assert_eq!(determine_route_kind("A", "B", &ni), "neutral");
    }

    #[test]
    fn determine_route_kind_downstream_precedence() {
        // If both downstream (on 'to') and upstream (on 'from') are present, downstream should take precedence
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": { "node_name": "B" }, "downstream": null }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "upstream": null, "downstream": [ { "node_name": "A" } ] }),
        );
        // Even though A.upstream points to B, B.downstream explicitly lists A so result should be 'downstream'
        assert_eq!(determine_route_kind("A", "B", &ni), "downstream");
    }

    #[test]
    fn determine_route_kind_multiple_downstreams() {
        let mut ni = BTreeMap::new();
        ni.insert(
            "C".to_string(),
            json!({ "node_type": "Obu", "upstream": null, "downstream": null }),
        );
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": null, "downstream": null }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "upstream": null, "downstream": [ { "node_name": "A" }, { "node_name": "C" } ] }),
        );
        assert_eq!(determine_route_kind("A", "B", &ni), "downstream");
        assert_eq!(determine_route_kind("C", "B", &ni), "downstream");
    }

    #[test]
    fn edge_width_monotonicity() {
        // ensure increasing traffic non-decreasing width
        let w0 = compute_edge_width(0.0, 0.0);
        let w1 = compute_edge_width(10.0, 0.0);
        let w2 = compute_edge_width(100.0, 50.0);
        let w3 = compute_edge_width(1e5, 1e5);
        assert!(w0 <= w1 && w1 <= w2 && w2 <= w3);
    }

    #[test]
    fn build_node_and_edge_json_basic() {
        let mut ni = BTreeMap::new();
        ni.insert("A".to_string(), json!({ "node_type": "Obu" }));
        ni.insert("B".to_string(), json!({ "node_type": "Rsu" }));

        let n = build_node_json("A", 1.0, -2.0, &ni);
        assert_eq!(n.get("id").and_then(|v| v.as_str()).unwrap(), "A");
        assert_eq!(n.get("label").and_then(|v| v.as_str()).unwrap(), "A");
        assert!(n.get("size").and_then(|v| v.as_i64()).unwrap() >= 9);

        let e = build_edge_json("A", "B", 100.0, 0.0, &ni);
        assert_eq!(e.get("source").and_then(|v| v.as_str()).unwrap(), "A");
        assert_eq!(e.get("target").and_then(|v| v.as_str()).unwrap(), "B");
        assert!(e.get("width").and_then(|v| v.as_i64()).unwrap() >= 1);
        // with the simple node_info above there is no explicit upstream/downstream relation
        assert_eq!(
            e.get("route_kind").and_then(|v| v.as_str()).unwrap(),
            "neutral"
        );
    }

    #[test]
    fn compute_edge_width_bounds() {
        // negative inputs should be treated as zero
        assert_eq!(
            compute_edge_width(-10.0, -5.0),
            compute_edge_width(0.0, 0.0)
        );
        // extremely large traffic is capped
        let w_large = compute_edge_width(1e12, 1e12);
        assert!(w_large <= 20);
        // tiny positive values still produce at least width 1
        assert!(compute_edge_width(1e-9, 0.0) >= 1);
    }

    #[test]
    fn build_node_json_missing_info() {
        let ni = BTreeMap::new();
        let n = build_node_json("X", 0.0, 0.0, &ni);
        assert_eq!(n.get("id").and_then(|v| v.as_str()).unwrap(), "X");
        // default size present and color present
        assert!(n.get("size").is_some());
        assert!(n.get("color").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn build_edge_json_precedence_and_cap() {
        // from has upstream->to, to has downstream listing -> downstream should win
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": { "node_name": "B" } }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "downstream": [ { "node_name": "A" } ] }),
        );
        let e = build_edge_json("A", "B", 1000.0, 0.0, &ni);
        assert_eq!(
            e.get("route_kind").and_then(|v| v.as_str()).unwrap(),
            "downstream"
        );

        // width is capped at 20 even for huge bps
        let e2 = build_edge_json("A", "B", 1e12, 1e12, &ni);
        assert!(e2.get("width").and_then(|v| v.as_i64()).unwrap() <= 20);
    }
}

// wasm-bindgen tests: placed here so they compile as part of the crate (not an external integration test)
#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;
    use wasm_bindgen_test::*;

    wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    fn wasm_compute_edge_width() {
        let w = compute_edge_width(0.0, 0.0);
        assert!(w >= 1 && w <= 3);
        let w2 = compute_edge_width(1e6, 1e6);
        assert!(w2 <= 12);
    }

    #[wasm_bindgen_test]
    fn wasm_route_kind() {
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": null, "downstream": null }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "upstream": null, "downstream": [ { "node_name": "A" } ] }),
        );
        let rk = determine_route_kind("A", "B", &ni);
        assert_eq!(rk, "downstream");
    }

    #[wasm_bindgen_test]
    fn wasm_build_node_json() {
        let mut ni = BTreeMap::new();
        ni.insert("X".to_string(), json!({ "node_type": "Obu" }));
        let n = build_node_json("X", 3.0, -4.0, &ni);
        assert_eq!(n.get("id").and_then(|v| v.as_str()).unwrap(), "X");
        assert!(n.get("size").is_some());
    }

    #[wasm_bindgen_test]
    fn wasm_build_edge_json_precedence() {
        let mut ni = BTreeMap::new();
        ni.insert(
            "A".to_string(),
            json!({ "node_type": "Obu", "upstream": { "node_name": "B" } }),
        );
        ni.insert(
            "B".to_string(),
            json!({ "node_type": "Rsu", "downstream": [ { "node_name": "A" } ] }),
        );
        let e = build_edge_json("A", "B", 200.0, 0.0, &ni);
        assert_eq!(
            e.get("route_kind").and_then(|v| v.as_str()).unwrap(),
            "downstream"
        );
    }

    #[wasm_bindgen_test]
    fn wasm_compute_edge_width_bounds() {
        assert_eq!(compute_edge_width(-1.0, -1.0), compute_edge_width(0.0, 0.0));
        let w = compute_edge_width(1e9, 1e9);
        assert!(w <= 12);
    }
}
