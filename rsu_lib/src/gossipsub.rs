//! GossipSub heartbeat publisher for RSU nodes.
//!
//! Enabled by the `libp2p_gossipsub` feature.  Runs alongside the existing
//! raw-L2 heartbeat loop without replacing it.

use libp2p_vanet::spawn::{rsu_memory_port as vanet_rsu_memory_port, spawn_rsu_gossipsub_task};
use mac_address::MacAddress;
use node_lib::Shared;

use crate::control::routing::Routing;

/// Derive the MemoryTransport port for this RSU's MAC address.
///
/// OBUs use this to determine where to dial for GossipSub heartbeats.
pub fn memory_port(mac: MacAddress) -> u64 {
    vanet_rsu_memory_port(mac)
}

/// Spawn a GossipSub heartbeat-publishing task alongside this RSU's existing
/// raw-L2 heartbeat loop.
///
/// The task listens on `/memory/<memory_port(mac)>`.  OBUs derive this
/// address via `gossipsub::memory_port(rsu_mac)`.
pub fn spawn_gossipsub_task(mac: MacAddress, routing: Shared<Routing>, periodicity_ms: u32) {
    spawn_rsu_gossipsub_task(mac, periodicity_ms, move || {
        let mut r = routing.write().expect("routing write lock");
        let msg = r.send_heartbeat(mac);
        let bytes: Vec<u8> = (&msg).into();
        bytes
    });
}
