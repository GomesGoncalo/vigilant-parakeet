//! GossipSub heartbeat subscriber for OBU nodes.
//!
//! Enabled by the `libp2p_gossipsub` feature.  Runs alongside the existing
//! raw-L2 heartbeat processing loop without replacing it.

use libp2p_vanet::spawn::spawn_obu_gossipsub_task;
use mac_address::MacAddress;
use node_lib::{
    messages::{message::Message, packet_type::PacketType},
    Shared,
};

use crate::control::routing::Routing;

/// Spawn a GossipSub heartbeat-receiving task alongside this OBU's existing
/// raw-L2 wire traffic loop.
///
/// `rsu_memory_port` should be derived via
/// `libp2p_vanet::spawn::rsu_memory_port(rsu_mac)`.
pub fn spawn_gossipsub_task(obu_mac: MacAddress, routing: Shared<Routing>, rsu_memory_port: u64) {
    spawn_obu_gossipsub_task(rsu_memory_port, move |bytes: Vec<u8>| {
        let msg = match Message::try_from(bytes.as_slice()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(error = %e, "GossipSub: failed to parse heartbeat bytes");
                return;
            }
        };
        if matches!(
            msg.get_packet_type(),
            PacketType::Control(node_lib::messages::control::Control::Heartbeat(_))
        ) {
            let mut r = routing.write().expect("routing write lock");
            let _ = r.handle_heartbeat(&msg, obu_mac);
        }
    });
}
