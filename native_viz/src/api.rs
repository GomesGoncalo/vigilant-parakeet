use serde::Deserialize;
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

/// Geographic position of one node, as returned by `GET /positions`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NodePosition {
    pub lat: f64,
    pub lon: f64,
    pub speed: f64,
    pub bearing: f64,
    /// Optional destination coordinates for the node's current trip.
    pub dest_lat: Option<f64>,
    pub dest_lon: Option<f64>,
}

/// Upstream routing entry carried inside [`NodeInfo`].
#[derive(Debug, Clone, Deserialize, Default)]
pub struct UpstreamInfo {
    pub hops: u32,
    pub mac: String,
    pub node_name: Option<String>,
    /// RSSI observed by this OBU towards its upstream node, in dBm.
    /// Present only when the simulator runs with the `mobility` feature
    /// (fading model active).  Typical range: −40 (strong) to −100 (weak).
    pub rssi_dbm: Option<f32>,
    /// Link latency in microseconds as measured by the routing layer.
    pub latency_us: Option<u64>,
}

/// Routing / type info for one node, as returned by `GET /node_info`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct NodeInfo {
    pub node_type: String,
    pub mac: String,
    #[allow(dead_code)]
    pub cloud_ip: Option<String>,
    pub virtual_ip: Option<String>,
    pub has_session: bool,
    pub upstream: Option<UpstreamInfo>,
}

/// Everything the UI needs for one rendered frame.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub positions: HashMap<String, NodePosition>,
    pub node_info: HashMap<String, NodeInfo>,
    /// Timestamp (monotonic) of the last successful positions fetch.
    pub last_positions_at: Option<std::time::Instant>,
    /// VANET max range in metres, read from /fading endpoint.  None if fading
    /// is disabled or the endpoint is not yet reachable.
    pub max_range_m: Option<f64>,
}

/// Background polling loop.  Runs forever on its own OS thread.
///
/// * Positions are refreshed every 200 ms for smooth vehicle movement.
/// * Node info (type, upstream) is refreshed every 2 s (slower, less volatile).
/// * Fading config (max_range_m) is fetched once at startup, then every 30 s.
pub fn poll_loop(base_url: String, tx: SyncSender<Snapshot>) {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("failed to build HTTP client");

    let positions_url = format!("{base_url}/positions");
    let nodes_url = format!("{base_url}/node_info");
    let fading_url = format!("{base_url}/fading");

    let mut snapshot = Snapshot::default();
    let positions_interval = Duration::from_millis(200);
    let node_info_interval = Duration::from_secs(2);
    let fading_interval = Duration::from_secs(30);

    let mut next_node_info = std::time::Instant::now();
    let mut next_fading = std::time::Instant::now();

    loop {
        let tick_start = std::time::Instant::now();

        // Fetch positions on every tick.
        if let Ok(resp) = client.get(&positions_url).send() {
            if let Ok(positions) = resp.json::<HashMap<String, NodePosition>>() {
                snapshot.positions = positions;
                snapshot.last_positions_at = Some(std::time::Instant::now());
            }
        }

        // Fetch node info on a slower cadence.
        if tick_start >= next_node_info {
            if let Ok(resp) = client.get(&nodes_url).send() {
                if let Ok(info) = resp.json::<HashMap<String, NodeInfo>>() {
                    snapshot.node_info = info;
                }
            }
            next_node_info = tick_start + node_info_interval;
        }

        // Fetch fading config (for correct RSU range circle radius).
        if tick_start >= next_fading {
            if let Ok(resp) = client.get(&fading_url).send() {
                if let Ok(v) = resp.json::<serde_json::Value>() {
                    snapshot.max_range_m = v.get("max_range_m").and_then(|x| x.as_f64());
                }
            }
            next_fading = tick_start + fading_interval;
        }

        // Best-effort send; if the UI is busy the oldest snapshot is dropped.
        let _ = tx.try_send(snapshot.clone());

        let elapsed = tick_start.elapsed();
        if elapsed < positions_interval {
            std::thread::sleep(positions_interval - elapsed);
        }
    }
}
