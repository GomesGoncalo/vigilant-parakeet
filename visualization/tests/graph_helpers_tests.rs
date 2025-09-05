use serde_json::json;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

// Local copy of bezier_points to test behavior without linking to the wasm crate.
fn bezier_points(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    offset: f64,
    samples: usize,
) -> (Vec<f64>, Vec<f64>) {
    let mx = (x0 + x1) / 2.0;
    let my = (y0 + y1) / 2.0;
    let dx = x1 - x0;
    let dy = y1 - y0;
    let dist = (dx * dx + dy * dy).sqrt().max(1.0);
    let nx = -dy / dist;
    let ny = dx / dist;
    let cx = mx + nx * offset;
    let cy = my + ny * offset;
    let mut xs = Vec::with_capacity(samples + 1);
    let mut ys = Vec::with_capacity(samples + 1);
    for i in 0..=samples {
        let t = (i as f64) / (samples as f64);
        let omt = 1.0 - t;
        let bx = omt * omt * x0 + 2.0 * omt * t * cx + t * t * x1;
        let by = omt * omt * y0 + 2.0 * omt * t * cy + t * t * y1;
        xs.push(bx);
        ys.push(by);
    }
    (xs, ys)
}

// Local copy of pick_node_type
fn pick_node_type(map: &HashMap<String, JsonValue>, node: &str) -> Option<String> {
    map.get(node).and_then(|ni| {
        ni.get("node_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    })
}

#[test]
fn bezier_points_endpoints_and_count() {
    let (xs, ys) = bezier_points(0.0, 0.0, 10.0, 0.0, 0.0, 10);
    // samples=10 => length 11
    assert_eq!(xs.len(), 11);
    assert_eq!(ys.len(), 11);
    // endpoints match inputs
    assert!((xs[0] - 0.0).abs() < 1e-12);
    assert!((ys[0] - 0.0).abs() < 1e-12);
    assert!((xs[10] - 10.0).abs() < 1e-12);
    assert!((ys[10] - 0.0).abs() < 1e-12);
}

#[test]
fn bezier_points_offset_changes_midpoint() {
    // compare midpoint with zero offset vs positive offset
    let (xs0, ys0) = bezier_points(0.0, 0.0, 10.0, 0.0, 0.0, 20);
    let (xs1, ys1) = bezier_points(0.0, 0.0, 10.0, 0.0, 5.0, 20);
    let mid0 = (xs0[10], ys0[10]);
    let mid1 = (xs1[10], ys1[10]);
    // with offset, midpoint y should differ (be non-zero)
    assert_eq!(mid0.0, mid1.0); // x midpoint for symmetric endpoints remains the same
    assert!((mid0.1 - mid1.1).abs() > 1e-6);
}

#[test]
fn bezier_points_linear_when_offset_zero() {
    // when offset is zero for straight horizontal endpoints, points should lie on the straight line y=0
    let (xs, ys) = bezier_points(-5.0, 2.0, 5.0, 2.0, 0.0, 8);
    for y in ys {
        assert!((y - 2.0).abs() < 1e-12);
    }
    // xs should be monotonic increasing
    for i in 1..xs.len() {
        assert!(xs[i] >= xs[i - 1]);
    }
}

#[test]
fn pick_node_type_returns_expected() {
    let mut m: HashMap<String, JsonValue> = HashMap::new();
    m.insert("n1".to_string(), json!({ "node_type": "Rsu", "other": 1 }));
    m.insert("n2".to_string(), json!({ "node_type": "Obu" }));
    m.insert("n3".to_string(), json!({ "no_type": true }));

    assert_eq!(pick_node_type(&m, "n1"), Some("Rsu".to_string()));
    assert_eq!(pick_node_type(&m, "n2"), Some("Obu".to_string()));
    assert_eq!(pick_node_type(&m, "n3"), None);
    assert_eq!(pick_node_type(&m, "missing"), None);
}
