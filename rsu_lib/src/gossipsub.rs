//! GossipSub heartbeat publisher for RSU nodes.
//!
//! Enabled by the `libp2p_gossipsub` feature.  Runs alongside the existing
//! raw-L2 heartbeat loop without replacing it.

use libp2p_vanet::spawn::{spawn_gossipsub_bootstrap, spawn_rsu_gossipsub_task};
use mac_address::MacAddress;
use node_lib::Shared;

use crate::control::routing::Routing;

/// Start the shared in-process GossipSub bootstrap/relay node.
///
/// Must be called **once** before any RSU or OBU GossipSub tasks are spawned.
/// The bootstrap node listens on a well-known MemoryTransport port and relays
/// heartbeats between RSU publishers and OBU subscribers, so no node needs to
/// know another node's address ahead of time.
pub fn start_bootstrap() {
    spawn_gossipsub_bootstrap();
}

/// Spawn a GossipSub heartbeat-publishing task alongside this RSU's existing
/// raw-L2 heartbeat loop.
///
/// The task dials the shared in-process bootstrap relay to join the GossipSub
/// mesh and publishes heartbeats every `periodicity_ms` milliseconds.
pub fn spawn_gossipsub_task(mac: MacAddress, routing: Shared<Routing>, periodicity_ms: u32) {
    spawn_rsu_gossipsub_task(mac, periodicity_ms, move || {
        let mut r = routing.write().expect("routing write lock");
        let msg = r.send_heartbeat(mac);
        let bytes: Vec<u8> = (&msg).into();
        bytes
    });
}
