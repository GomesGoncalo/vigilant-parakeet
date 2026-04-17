//! High-level spawn helpers for RSU and OBU GossipSub tasks.
//!
//! These functions keep all libp2p imports inside `libp2p_vanet`, so callers
//! (rsu_lib, obu_lib) only need to depend on this crate — not on libp2p directly.

use libp2p::{
    core::{transport::MemoryTransport, upgrade::Version},
    futures::StreamExt,
    gossipsub::IdentTopic,
    identity::Keypair,
    noise,
    swarm::SwarmEvent,
    yamux, SwarmBuilder, Transport,
};
use mac_address::MacAddress;
use std::time::Duration;

use crate::behaviour::{VanetBehaviour, VanetBehaviourEvent, HEARTBEAT_TOPIC};

/// Spawn a GossipSub heartbeat-publishing task for an RSU node.
///
/// `get_heartbeat` is called on every `periodicity_ms` tick and should return
/// the full wire bytes for the current heartbeat (serialised using the existing
/// VANET protocol).  Bytes are published to `vanet/heartbeat/v1`.
///
/// The task listens on `/memory/<mac_as_u64>` so OBUs can derive the address
/// deterministically from the RSU MAC address.
pub fn spawn_rsu_gossipsub_task(
    mac: MacAddress,
    periodicity_ms: u32,
    get_heartbeat: impl Fn() -> Vec<u8> + Send + 'static,
) {
    let keypair = Keypair::generate_ed25519();
    let port = mac_to_u64(mac);
    let topic = IdentTopic::new(HEARTBEAT_TOPIC);
    let topic_clone = topic.clone();

    tokio::spawn(async move {
        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_other_transport(|key| {
                let noise = noise::Config::new(key).expect("noise");
                MemoryTransport::default()
                    .upgrade(Version::V1Lazy)
                    .authenticate(noise)
                    .multiplex(yamux::Config::default())
                    .boxed()
            })
            .expect("transport")
            .with_behaviour(|key| VanetBehaviour::new(key, &[topic_clone]))
            .expect("behaviour")
            .build();

        let listen_addr = format!("/memory/{port}").parse().expect("addr");
        if let Err(e) = swarm.listen_on(listen_addr) {
            tracing::error!(error = %e, "RSU GossipSub listen failed");
            return;
        }

        let mut interval = tokio::time::interval(Duration::from_millis(u64::from(periodicity_ms)));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let bytes = get_heartbeat();
                    if let Err(e) = swarm.behaviour_mut().gossipsub.publish(topic.clone(), bytes) {
                        tracing::debug!(error = %e, "GossipSub publish skipped (no peers yet)");
                    }
                }
                event = swarm.select_next_some() => {
                    if let SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(ev)) = event {
                        tracing::trace!(?ev, "RSU GossipSub event");
                    }
                }
            }
        }
    });
}

/// Spawn a GossipSub heartbeat-receiving task for an OBU node.
///
/// Dials the RSU at `/memory/<rsu_port>` (derive via `rsu_memory_port(rsu_mac)`),
/// subscribes to `vanet/heartbeat/v1`, and calls `handle_heartbeat` with raw
/// heartbeat bytes for each received message.
pub fn spawn_obu_gossipsub_task(
    rsu_memory_port: u64,
    handle_heartbeat: impl Fn(Vec<u8>) + Send + 'static,
) {
    let keypair = Keypair::generate_ed25519();
    let topic = IdentTopic::new(HEARTBEAT_TOPIC);
    let topic_clone = topic.clone();

    tokio::spawn(async move {
        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_other_transport(|key| {
                let noise = noise::Config::new(key).expect("noise");
                MemoryTransport::default()
                    .upgrade(Version::V1Lazy)
                    .authenticate(noise)
                    .multiplex(yamux::Config::default())
                    .boxed()
            })
            .expect("transport")
            .with_behaviour(|key| VanetBehaviour::new(key, &[topic_clone]))
            .expect("behaviour")
            .build();

        let rsu_addr: libp2p::Multiaddr =
            format!("/memory/{rsu_memory_port}").parse().expect("addr");
        if let Err(e) = swarm.dial(rsu_addr) {
            tracing::warn!(error = %e, "OBU GossipSub dial RSU failed");
            return;
        }

        loop {
            let event = swarm.select_next_some().await;
            if let SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(
                libp2p::gossipsub::Event::Message { message, .. },
            )) = event
            {
                handle_heartbeat(message.data);
            }
        }
    });
}

/// Derive the MemoryTransport port for a MAC address.
///
/// OBUs use this to determine the RSU's listen address without extra
/// configuration, mirroring how in a real L2 deployment the RSU MAC directly
/// identifies the L2 endpoint.
pub fn rsu_memory_port(mac: MacAddress) -> u64 {
    mac_to_u64(mac)
}

fn mac_to_u64(mac: MacAddress) -> u64 {
    let b = mac.bytes();
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], 0, 0])
}
