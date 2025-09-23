use common::channel_parameters::ChannelParameters;
use serde_json::Value as JsonValue;
use serde_wasm_bindgen::to_value;
use std::collections::{BTreeMap, HashMap};
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::{console, window};
use yew::prelude::*;

#[derive(Properties, PartialEq, Clone)]
pub struct GraphProps {
    pub nodes: Vec<String>,
    pub channels: HashMap<String, HashMap<String, ChannelParameters>>,
    // node_info is passed as opaque JSON so the library build doesn't need
    // the binary-local NodeInfo struct. This makes the crate usable as a
    // library for wasm tests.
    pub node_info: std::collections::HashMap<String, JsonValue>,
    pub stats: std::collections::HashMap<String, JsonValue>,
}

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

fn pick_node_type(
    map: &std::collections::HashMap<String, JsonValue>,
    node: &str,
) -> Option<String> {
    map.get(node).and_then(|ni| {
        ni.get("node_type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    })
}

#[function_component(Graph)]
pub fn graph(props: &GraphProps) -> Html {
    let nodes = props.nodes.clone();
    let channels = props.channels.clone();
    let node_info = props.node_info.clone();
    let stats = props.stats.clone();

    // persistent previous snapshot for delta computation (bytes-like sums)
    let last_snapshot = use_state(std::collections::BTreeMap::<String, f64>::new);
    let last_time = use_state(|| 0f64);

    // effect: compute positions and call Plotly
    use_effect_with(
        (
            nodes.clone(),
            channels.clone(),
            node_info.clone(),
            stats.clone(),
        ),
        move |(_n, _c, _u, _s)| {
            // compute RSUs and OBUs
            let mut rsus = Vec::new();
            let mut obus = Vec::new();
            for n in &nodes {
                if let Some(t) = pick_node_type(&node_info, n) {
                    if t.to_lowercase() == "rsu" {
                        rsus.push(n.clone());
                        continue;
                    }
                }
                obus.push(n.clone());
            }

            let mut positions: BTreeMap<String, (f64, f64)> = BTreeMap::new();
            let rsu_count = std::cmp::max(1, rsus.len());
            for (i, r) in rsus.iter().enumerate() {
                let x = (i as f64 - (rsu_count as f64 - 1.0) / 2.0) * 2.0;
                positions.insert(r.clone(), (x, 1.5));
            }
            let obu_count = obus.len();
            let radius = (1.0 + (obu_count as f64) / 6.0).max(1.5);
            for (i, o) in obus.iter().enumerate() {
                let angle = (2.0 * std::f64::consts::PI * (i as f64))
                    / (if obu_count > 0 { obu_count as f64 } else { 1.0 });
                let x = angle.cos() * radius;
                let y = -1.0 + angle.sin() * (radius * 0.25);
                positions.insert(o.clone(), (x, y));
            }

            // nudge OBUs toward upstream RSU if available
            for o in &obus {
                if let Some(info) = node_info.get(o) {
                    if let Some(up) = info.get("upstream") {
                        if let Some(un) = up.get("node_name").and_then(|v| v.as_str()) {
                            if let (Some((ox, oy)), Some((ux, _uy))) =
                                (positions.get(o).cloned(), positions.get(un).cloned())
                            {
                                let newx = (ox + ux) / 2.0;
                                positions.insert(o.clone(), (newx, oy));
                            }
                        }
                    }
                }
            }

            // derive edge list (from->to) using channels map; if empty, derive from upstream
            let mut edges = Vec::new();
            if !channels.is_empty() {
                for (from, inner) in &channels {
                    for to in inner.keys() {
                        edges.push((from.clone(), to.clone()));
                    }
                }
            } else {
                for (n, info) in &node_info {
                    // node_info is JSON-like here; read upstream via get("upstream")
                    if let Some(up) = info.get("upstream") {
                        if let Some(un) = up.get("node_name").and_then(|v| v.as_str()) {
                            edges.push((n.clone(), un.to_string()));
                        }
                    }
                }
            }

            // compute per-node numeric sums from stats (to be used for deltas)
            let mut node_sums: BTreeMap<String, f64> = BTreeMap::new();
            for (node, val) in &stats {
                let mut sum = 0f64;
                match val {
                    JsonValue::Object(map) => {
                        for (_k, v) in map {
                            if let JsonValue::Object(o) = v {
                                for (_k2, v2) in o {
                                    if let JsonValue::Number(n) = v2 {
                                        sum += n.as_f64().unwrap_or(0.0);
                                    }
                                }
                            } else if let JsonValue::Number(n) = v {
                                sum += n.as_f64().unwrap_or(0.0);
                            }
                        }
                    }
                    JsonValue::Array(arr) => {
                        for item in arr {
                            if let JsonValue::Object(o) = item {
                                for (_k, v) in o {
                                    if let JsonValue::Number(n) = v {
                                        sum += n.as_f64().unwrap_or(0.0);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
                node_sums.insert(node.clone(), sum);
            }

            // compute deltas (bps) from last snapshot
            let now_ms = js_sys::Date::now();
            let now_s = now_ms / 1000.0;
            let mut node_bps: BTreeMap<String, f64> = BTreeMap::new();
            {
                let prev = &*last_snapshot;
                let prev_t = *last_time;
                let dt = if prev_t > 0.0 { now_s - prev_t } else { 0.0 };
                for (node, &cur) in node_sums.iter() {
                    let bps = if dt > 0.0 {
                        let prev_v = *prev.get(node).unwrap_or(&0.0);
                        let delta = (cur - prev_v).max(0.0);
                        delta / dt
                    } else {
                        0.0
                    };
                    node_bps.insert(node.clone(), bps);
                }
            }
            // update snapshot for next run
            {
                let mut ns = (*last_snapshot).clone();
                for (k, v) in node_sums.into_iter() {
                    ns.insert(k, v);
                }
                last_snapshot.set(ns);
                last_time.set(now_s);
            }

            // Prepare arrays for Plotly traces
            let mut data_traces: Vec<JsonValue> = Vec::new();

            // Build annotations for each edge (arrow from src->dst). Color by presence of tx bytes.
            let mut annotations: Vec<JsonValue> = Vec::new();
            for (from, to) in &edges {
                if let (Some((sx, sy)), Some((tx, ty))) =
                    (positions.get(from).cloned(), positions.get(to).cloned())
                {
                    // baseline straight faint line (for context)
                    let mut baseline = serde_json::Map::new();
                    baseline.insert(
                        "x".to_string(),
                        JsonValue::Array(vec![
                            JsonValue::Number(serde_json::Number::from_f64(sx).unwrap()),
                            JsonValue::Number(serde_json::Number::from_f64(tx).unwrap()),
                        ]),
                    );
                    baseline.insert(
                        "y".to_string(),
                        JsonValue::Array(vec![
                            JsonValue::Number(serde_json::Number::from_f64(sy).unwrap()),
                            JsonValue::Number(serde_json::Number::from_f64(ty).unwrap()),
                        ]),
                    );
                    baseline.insert("mode".to_string(), JsonValue::String("lines".to_string()));
                    baseline.insert(
                        "line".to_string(),
                        JsonValue::Object({
                            let mut m = serde_json::Map::new();
                            m.insert("color".to_string(), JsonValue::String("#333".to_string()));
                            m.insert(
                                "width".to_string(),
                                JsonValue::Number(serde_json::Number::from(1)),
                            );
                            m.insert("dash".to_string(), JsonValue::String("dot".to_string()));
                            m
                        }),
                    );
                    baseline.insert(
                        "hoverinfo".to_string(),
                        JsonValue::String("none".to_string()),
                    );
                    baseline.insert("showlegend".to_string(), JsonValue::Bool(false));
                    baseline.insert("type".to_string(), JsonValue::String("scatter".to_string()));
                    data_traces.push(JsonValue::Object(baseline));

                    // compute a simple up/down indicator using node_bps heuristics
                    let up_bps = *node_bps.get(from).unwrap_or(&0.0);
                    let down_bps = *node_bps.get(to).unwrap_or(&0.0);

                    // choose offsets
                    let dx = tx - sx;
                    let dy = ty - sy;
                    let dist = (dx * dx + dy * dy).sqrt().max(1.0);
                    let offset_scale = |v: f64| ((v.max(1.0).ln().abs() + 1.0) * 0.02).min(0.25);

                    if up_bps > 0.0 {
                        let offset = dist * offset_scale(up_bps);
                        let (xs, ys) = bezier_points(sx, sy, tx, ty, offset, 30);
                        let mut trace = serde_json::Map::new();
                        trace.insert(
                            "x".to_string(),
                            JsonValue::Array(
                                xs.into_iter()
                                    .map(|v| {
                                        JsonValue::Number(serde_json::Number::from_f64(v).unwrap())
                                    })
                                    .collect(),
                            ),
                        );
                        trace.insert(
                            "y".to_string(),
                            JsonValue::Array(
                                ys.into_iter()
                                    .map(|v| {
                                        JsonValue::Number(serde_json::Number::from_f64(v).unwrap())
                                    })
                                    .collect(),
                            ),
                        );
                        trace.insert("mode".to_string(), JsonValue::String("lines".to_string()));
                        trace.insert(
                            "hoverinfo".to_string(),
                            JsonValue::String("text".to_string()),
                        );
                        trace.insert("text".to_string(), JsonValue::Array(vec![]));
                        trace.insert(
                            "line".to_string(),
                            JsonValue::Object({
                                let mut m = serde_json::Map::new();
                                m.insert(
                                    "color".to_string(),
                                    JsonValue::String("rgba(200,30,30,0.9)".to_string()),
                                );
                                m.insert(
                                    "width".to_string(),
                                    JsonValue::Number(serde_json::Number::from(2)),
                                );
                                m
                            }),
                        );
                        trace.insert("showlegend".to_string(), JsonValue::Bool(false));
                        trace.insert("type".to_string(), JsonValue::String("scatter".to_string()));
                        data_traces.push(JsonValue::Object(trace));
                        // midpoint marker
                        let mx = (sx + tx) / 2.0;
                        let my = (sy + ty) / 2.0;
                        let mut mid = serde_json::Map::new();
                        mid.insert(
                            "x".to_string(),
                            JsonValue::Array(vec![JsonValue::Number(
                                serde_json::Number::from_f64(mx).unwrap(),
                            )]),
                        );
                        mid.insert(
                            "y".to_string(),
                            JsonValue::Array(vec![JsonValue::Number(
                                serde_json::Number::from_f64(my).unwrap(),
                            )]),
                        );
                        mid.insert("mode".to_string(), JsonValue::String("markers".to_string()));
                        mid.insert("type".to_string(), JsonValue::String("scatter".to_string()));
                        mid.insert(
                            "marker".to_string(),
                            JsonValue::Object({
                                let mut mm = serde_json::Map::new();
                                mm.insert(
                                    "size".to_string(),
                                    JsonValue::Number(serde_json::Number::from(8)),
                                );
                                mm.insert(
                                    "color".to_string(),
                                    JsonValue::String("rgba(200,30,30,0.8)".to_string()),
                                );
                                mm
                            }),
                        );
                        mid.insert(
                            "text".to_string(),
                            JsonValue::Array(vec![JsonValue::String(format!(
                                "{} → {}\nup: {} bps",
                                from, to, up_bps as i64
                            ))]),
                        );
                        mid.insert(
                            "hoverinfo".to_string(),
                            JsonValue::String("text".to_string()),
                        );
                        mid.insert("showlegend".to_string(), JsonValue::Bool(false));
                        data_traces.push(JsonValue::Object(mid));
                    } else {
                        // faint arrow annotation for no traffic will be added below
                    }

                    if down_bps > 0.0 {
                        let offset = -dist * offset_scale(down_bps);
                        let (xs, ys) = bezier_points(sx, sy, tx, ty, offset, 30);
                        let mut trace = serde_json::Map::new();
                        trace.insert(
                            "x".to_string(),
                            JsonValue::Array(
                                xs.into_iter()
                                    .map(|v| {
                                        JsonValue::Number(serde_json::Number::from_f64(v).unwrap())
                                    })
                                    .collect(),
                            ),
                        );
                        trace.insert(
                            "y".to_string(),
                            JsonValue::Array(
                                ys.into_iter()
                                    .map(|v| {
                                        JsonValue::Number(serde_json::Number::from_f64(v).unwrap())
                                    })
                                    .collect(),
                            ),
                        );
                        trace.insert("mode".to_string(), JsonValue::String("lines".to_string()));
                        trace.insert(
                            "hoverinfo".to_string(),
                            JsonValue::String("text".to_string()),
                        );
                        trace.insert("text".to_string(), JsonValue::Array(vec![]));
                        trace.insert(
                            "line".to_string(),
                            JsonValue::Object({
                                let mut m = serde_json::Map::new();
                                m.insert(
                                    "color".to_string(),
                                    JsonValue::String("rgba(30,100,200,0.9)".to_string()),
                                );
                                m.insert(
                                    "width".to_string(),
                                    JsonValue::Number(serde_json::Number::from(2)),
                                );
                                m
                            }),
                        );
                        trace.insert("showlegend".to_string(), JsonValue::Bool(false));
                        trace.insert("type".to_string(), JsonValue::String("scatter".to_string()));
                        data_traces.push(JsonValue::Object(trace));
                        // midpoint marker
                        let mx = (sx + tx) / 2.0;
                        let my = (sy + ty) / 2.0;
                        let mut mid = serde_json::Map::new();
                        mid.insert(
                            "x".to_string(),
                            JsonValue::Array(vec![JsonValue::Number(
                                serde_json::Number::from_f64(mx).unwrap(),
                            )]),
                        );
                        mid.insert(
                            "y".to_string(),
                            JsonValue::Array(vec![JsonValue::Number(
                                serde_json::Number::from_f64(my).unwrap(),
                            )]),
                        );
                        mid.insert("mode".to_string(), JsonValue::String("markers".to_string()));
                        mid.insert("type".to_string(), JsonValue::String("scatter".to_string()));
                        mid.insert(
                            "marker".to_string(),
                            JsonValue::Object({
                                let mut mm = serde_json::Map::new();
                                mm.insert(
                                    "size".to_string(),
                                    JsonValue::Number(serde_json::Number::from(8)),
                                );
                                mm.insert(
                                    "color".to_string(),
                                    JsonValue::String("rgba(30,100,200,0.8)".to_string()),
                                );
                                mm
                            }),
                        );
                        mid.insert(
                            "text".to_string(),
                            JsonValue::Array(vec![JsonValue::String(format!(
                                "{} → {}\ndown: {} bps",
                                from, to, down_bps as i64
                            ))]),
                        );
                        mid.insert(
                            "hoverinfo".to_string(),
                            JsonValue::String("text".to_string()),
                        );
                        mid.insert("showlegend".to_string(), JsonValue::Bool(false));
                        data_traces.push(JsonValue::Object(mid));
                    }

                    // arrow annotations: create directed arrow(s)
                    // helper to push an annotation given tail (ax,ay) and head (x,y)
                    let mut push_ann =
                        |ax: f64, ay: f64, x: f64, y: f64, color: &str, width: f64| {
                            let mut ann = serde_json::Map::new();
                            ann.insert(
                                "x".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(x).unwrap()),
                            );
                            ann.insert(
                                "y".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(y).unwrap()),
                            );
                            ann.insert(
                                "ax".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(ax).unwrap()),
                            );
                            ann.insert(
                                "ay".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(ay).unwrap()),
                            );
                            ann.insert("xref".to_string(), JsonValue::String("x".to_string()));
                            ann.insert("yref".to_string(), JsonValue::String("y".to_string()));
                            ann.insert("axref".to_string(), JsonValue::String("x".to_string()));
                            ann.insert("ayref".to_string(), JsonValue::String("y".to_string()));
                            ann.insert(
                                "arrowhead".to_string(),
                                JsonValue::Number(serde_json::Number::from(3)),
                            );
                            ann.insert(
                                "arrowsize".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(1.0).unwrap()),
                            );
                            ann.insert(
                                "arrowwidth".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(width).unwrap()),
                            );
                            ann.insert(
                                "arrowcolor".to_string(),
                                JsonValue::String(color.to_string()),
                            );
                            ann.insert(
                                "opacity".to_string(),
                                JsonValue::Number(serde_json::Number::from_f64(0.95).unwrap()),
                            );
                            annotations.push(JsonValue::Object(ann));
                        };

                    // if we drew an up-curve, place arrow along that bezier near destination
                    if up_bps > 0.0 {
                        let (xs2, ys2) =
                            bezier_points(sx, sy, tx, ty, dist * offset_scale(up_bps), 30);
                        if xs2.len() >= 3 {
                            let last = xs2.len() - 1;
                            let ax = xs2[last.saturating_sub(2)];
                            let ay = ys2[last.saturating_sub(2)];
                            let xh = xs2[last];
                            let yh = ys2[last];
                            push_ann(ax, ay, xh, yh, "rgba(200,30,30,0.95)", 2.0);
                        }
                    }
                    // if we drew a down-curve, place arrow along that bezier near source->dest but opposite offset
                    if down_bps > 0.0 {
                        let (xs2, ys2) =
                            bezier_points(sx, sy, tx, ty, -dist * offset_scale(down_bps), 30);
                        if xs2.len() >= 3 {
                            let last = xs2.len() - 1;
                            let ax = xs2[last.saturating_sub(2)];
                            let ay = ys2[last.saturating_sub(2)];
                            let xh = xs2[last];
                            let yh = ys2[last];
                            push_ann(ax, ay, xh, yh, "rgba(30,100,200,0.95)", 2.0);
                        }
                    }
                    // if neither direction has traffic, show a faint single arrow from from->to
                    if up_bps <= 0.0 && down_bps <= 0.0 {
                        // small arrow near midpoint
                        let ax = sx * 0.55 + tx * 0.45;
                        let ay = sy * 0.55 + ty * 0.45;
                        let xh = sx * 0.8 + tx * 0.2;
                        let yh = sy * 0.8 + ty * 0.2;
                        push_ann(ax, ay, xh, yh, "rgba(120,120,120,0.6)", 1.0);
                    }
                }
            }

            // add node markers (draw on top of traces)
            {
                let mut xs: Vec<JsonValue> = Vec::new();
                let mut ys: Vec<JsonValue> = Vec::new();
                let mut texts: Vec<JsonValue> = Vec::new();
                let mut colors: Vec<JsonValue> = Vec::new();
                let mut sizes: Vec<JsonValue> = Vec::new();
                for (name, (x, y)) in &positions {
                    xs.push(JsonValue::Number(serde_json::Number::from_f64(*x).unwrap()));
                    ys.push(JsonValue::Number(serde_json::Number::from_f64(*y).unwrap()));
                    let label = if let Some(nt) = pick_node_type(&node_info, name) {
                        format!("{name} ({nt})")
                    } else {
                        name.clone()
                    };
                    texts.push(JsonValue::String(label));
                    if let Some(nt) = pick_node_type(&node_info, name) {
                        if nt.to_lowercase() == "rsu" {
                            colors.push(JsonValue::String("rgba(200,30,30,0.95)".to_string()));
                            sizes.push(JsonValue::Number(serde_json::Number::from(14)));
                        } else {
                            colors.push(JsonValue::String("rgba(30,100,200,0.95)".to_string()));
                            sizes.push(JsonValue::Number(serde_json::Number::from(10)));
                        }
                    } else {
                        colors.push(JsonValue::String("rgba(100,100,100,0.9)".to_string()));
                        sizes.push(JsonValue::Number(serde_json::Number::from(9)));
                    }
                }
                let mut node_trace = serde_json::Map::new();
                node_trace.insert("x".to_string(), JsonValue::Array(xs));
                node_trace.insert("y".to_string(), JsonValue::Array(ys));
                node_trace.insert(
                    "mode".to_string(),
                    JsonValue::String("markers+text".to_string()),
                );
                node_trace.insert("text".to_string(), JsonValue::Array(texts));
                node_trace.insert(
                    "textposition".to_string(),
                    JsonValue::String("top center".to_string()),
                );
                node_trace.insert(
                    "hoverinfo".to_string(),
                    JsonValue::String("text".to_string()),
                );
                node_trace.insert("showlegend".to_string(), JsonValue::Bool(false));
                node_trace.insert("type".to_string(), JsonValue::String("scatter".to_string()));
                node_trace.insert(
                    "marker".to_string(),
                    JsonValue::Object({
                        let mut m = serde_json::Map::new();
                        m.insert("color".to_string(), JsonValue::Array(colors));
                        m.insert("size".to_string(), JsonValue::Array(sizes));
                        m
                    }),
                );
                data_traces.push(JsonValue::Object(node_trace));
            }

            // logging for debug (filterable)
            {
                let tag = "[VP-GRAPH]";
                let pos_json = serde_json::to_string(&positions)
                    .unwrap_or_else(|_| "<pos-serde-err>".to_string());
                console::log_1(&JsValue::from_str(&format!("{tag} positions: {pos_json}")));
                let bps_json = serde_json::to_string(&node_bps)
                    .unwrap_or_else(|_| "<bps-serde-err>".to_string());
                console::log_1(&JsValue::from_str(&format!("{tag} node_bps: {bps_json}")));
                console::log_1(&JsValue::from_str(&format!(
                    "{} data_traces_count: {}",
                    tag,
                    data_traces.len()
                )));
                let types: Vec<String> = data_traces
                    .iter()
                    .take(8)
                    .map(|t| match t {
                        JsonValue::Object(m) => m
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("obj")
                            .to_string(),
                        _ => "other".to_string(),
                    })
                    .collect();
                console::log_1(&JsValue::from_str(&format!("{tag} trace_types: {types:?}")));
            }

            // Build a minimal layout and attach data_traces (used downstream in JS rendering)
            let mut layout_map = serde_json::Map::new();
            layout_map.insert("annotations".to_string(), JsonValue::Array(annotations));
            layout_map.insert(
                "xaxis".to_string(),
                JsonValue::Object({
                    let mut mm = serde_json::Map::new();
                    mm.insert("showgrid".to_string(), JsonValue::Bool(false));
                    mm.insert("zeroline".to_string(), JsonValue::Bool(false));
                    mm.insert("showticklabels".to_string(), JsonValue::Bool(false));
                    mm.insert("fixedrange".to_string(), JsonValue::Bool(true));
                    mm
                }),
            );
            layout_map.insert(
                "yaxis".to_string(),
                JsonValue::Object({
                    let mut mm = serde_json::Map::new();
                    mm.insert("showgrid".to_string(), JsonValue::Bool(false));
                    mm.insert("zeroline".to_string(), JsonValue::Bool(false));
                    mm.insert("showticklabels".to_string(), JsonValue::Bool(false));
                    mm.insert("fixedrange".to_string(), JsonValue::Bool(true));
                    mm
                }),
            );
            layout_map.insert(
                "margin".to_string(),
                JsonValue::Object({
                    let mut mm = serde_json::Map::new();
                    mm.insert("l".to_string(), JsonValue::Number(0.into()));
                    mm.insert("r".to_string(), JsonValue::Number(0.into()));
                    mm.insert("t".to_string(), JsonValue::Number(0.into()));
                    mm.insert("b".to_string(), JsonValue::Number(0.into()));
                    mm
                }),
            );

            // Build nodes and edges payloads for Cytoscape interop
            if let Some(win) = window() {
                // convert node_info into BTreeMap<String, JsonValue> once for helpers
                let mut ni_map: std::collections::BTreeMap<String, JsonValue> =
                    std::collections::BTreeMap::new();
                for (k, v) in &node_info {
                    ni_map.insert(k.clone(), v.clone());
                }
                // Scale logical layout coordinates into pixel space for Cytoscape
                let position_scale = 120.0f64; // maps small layout coords (~-3..3) into visible pixels
                let mut nodes_arr: Vec<JsonValue> = Vec::new();
                for (name, (x, y)) in &positions {
                    let sx = x * position_scale;
                    let sy = y * position_scale;
                    let n = crate::graph_helpers::build_node_json(name, sx, sy, &ni_map);
                    nodes_arr.push(n);
                }

                let mut edges_arr: Vec<JsonValue> = Vec::new();
                for (from, to) in &edges {
                    let src = if from.is_empty() {
                        continue;
                    } else {
                        from.clone()
                    };
                    let tgt = if to.is_empty() {
                        continue;
                    } else {
                        to.clone()
                    };
                    let up = *node_bps.get(from).unwrap_or(&0.0);
                    let down = *node_bps.get(to).unwrap_or(&0.0);
                    let e = crate::graph_helpers::build_edge_json(&src, &tgt, up, down, &ni_map);
                    edges_arr.push(e);
                }

                // Ensure nodes have valid numeric positions and ids (defensive) before sending to JS
                for nval in nodes_arr.iter_mut() {
                    if let JsonValue::Object(map) = nval {
                        // ensure id exists
                        if !map.contains_key("id") {
                            if let Some(JsonValue::String(lbl)) = map.get("label") {
                                map.insert("id".to_string(), JsonValue::String(lbl.clone()));
                            }
                        }
                        // ensure position object exists and has numeric x/y
                        let mut px = None;
                        let mut py = None;
                        if let Some(JsonValue::Number(nx)) = map.get("x") {
                            px = nx.as_f64();
                        }
                        if let Some(JsonValue::Number(ny)) = map.get("y") {
                            py = ny.as_f64();
                        }
                        if let Some(JsonValue::Object(pos)) = map.get("position") {
                            // check nested position
                            if let Some(JsonValue::Number(nx)) = pos.get("x") {
                                px = px.or(nx.as_f64());
                            }
                            if let Some(JsonValue::Number(ny)) = pos.get("y") {
                                py = py.or(ny.as_f64());
                            }
                        }
                        let xval = px.unwrap_or(0.0);
                        let yval = py.unwrap_or(0.0);
                        let mut posmap = serde_json::Map::new();
                        posmap.insert(
                            "x".to_string(),
                            JsonValue::Number(serde_json::Number::from_f64(xval).unwrap()),
                        );
                        posmap.insert(
                            "y".to_string(),
                            JsonValue::Number(serde_json::Number::from_f64(yval).unwrap()),
                        );
                        map.insert("position".to_string(), JsonValue::Object(posmap));
                        // also keep top-level x/y as numbers
                        map.insert(
                            "x".to_string(),
                            JsonValue::Number(serde_json::Number::from_f64(xval).unwrap()),
                        );
                        map.insert(
                            "y".to_string(),
                            JsonValue::Number(serde_json::Number::from_f64(yval).unwrap()),
                        );
                    }
                }

                // debug: serialize nodes/edges to console so the browser shows exact payload
                console::log_1(&JsValue::from_str(&format!(
                    "[VP-GRAPH] nodes={} edges={}",
                    nodes_arr.len(),
                    edges_arr.len()
                )));

                // call window.__vp_render_graph(nodes, edges)
                // expose node_info to the page so the JS renderer can inspect upstream/downstream
                let _ = js_sys::Reflect::set(
                    &win,
                    &JsValue::from_str("__vp_node_info"),
                    &to_value(&ni_map).unwrap(),
                );
                let _ = js_sys::Reflect::set(
                    &win,
                    &JsValue::from_str("__vp_last_node_info"),
                    &to_value(&ni_map).unwrap(),
                );
                let render = js_sys::Reflect::get(&win, &JsValue::from_str("__vp_render_graph"));
                if let Ok(r) = render {
                    if r.is_function() {
                        let func: js_sys::Function = r.dyn_into().unwrap();
                        let args = js_sys::Array::new();
                        args.push(&to_value(&nodes_arr).unwrap());
                        args.push(&to_value(&edges_arr).unwrap());
                        let _ = func.apply(&JsValue::NULL, &args);
                    }
                }
            }

            || ()
        },
    );

    html! {
        <div id="graph" style="width:100%; min-height:480px;"></div>
    }
}
