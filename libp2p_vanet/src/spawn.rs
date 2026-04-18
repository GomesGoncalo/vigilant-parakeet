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
    swarm::{dial_opts::DialOpts, SwarmEvent},
    yamux, SwarmBuilder, Transport,
};
use mac_address::MacAddress;
use std::time::Duration;

use crate::behaviour::{VanetBehaviour, VanetBehaviourEvent, HEARTBEAT_TOPIC};

/// Fixed MemoryTransport port for the in-process GossipSub bootstrap/relay node.
///
/// This value is above any 6-byte MAC-derived port (`mac_to_u64` produces at
/// most a 48-bit value) so there is no collision with RSU listen addresses.
/// All RSU and OBU swarms dial this port to join the shared GossipSub mesh
/// without requiring peer-to-peer address configuration.
pub const BOOTSTRAP_PORT: u64 = 1u64 << 48;

/// Spawn the in-process GossipSub bootstrap/relay node.
///
/// The bootstrap node listens on `/memory/BOOTSTRAP_PORT`, subscribes to
/// `vanet/heartbeat/v1`, and relays messages so RSU publishers and OBU
/// subscribers can exchange heartbeats without knowing each other's addresses.
/// This is necessary because OBUs are mobile and cannot have a fixed RSU MAC
/// configured ahead of time.
///
/// Call this **once** at simulator startup, before spawning any RSU or OBU
/// GossipSub tasks.  RSU and OBU tasks retry failed dials automatically, so
/// a brief startup race is harmless.
pub fn spawn_gossipsub_bootstrap() {
    let keypair = Keypair::generate_ed25519();
    let topic = IdentTopic::new(HEARTBEAT_TOPIC);

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
            .with_behaviour(|key| VanetBehaviour::new_relay(key, &[topic]))
            .expect("behaviour")
            .build();

        let addr = format!("/memory/{BOOTSTRAP_PORT}").parse().expect("bootstrap addr");
        if let Err(e) = swarm.listen_on(addr) {
            tracing::error!(error = %e, "GossipSub bootstrap listen failed");
            return;
        }

        loop {
            swarm.select_next_some().await;
        }
    });
}

/// Spawn a GossipSub heartbeat-publishing task for an RSU node.
///
/// `get_heartbeat` is called on every `periodicity_ms` tick and should return
/// the full wire bytes for the current heartbeat (serialised using the existing
/// VANET protocol).  Bytes are published to `vanet/heartbeat/v1`.
///
/// The task also listens on `/memory/<mac_as_u64>` for potential direct
/// connections, and dials the bootstrap relay at `/memory/BOOTSTRAP_PORT`
/// to join the shared in-process GossipSub mesh.
pub fn spawn_rsu_gossipsub_task(
    mac: MacAddress,
    periodicity_ms: u32,
    get_heartbeat: impl Fn() -> Vec<u8> + Send + 'static,
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

        let listen_addr = format!("/memory/{}", mac_to_u64(mac)).parse().expect("addr");
        if let Err(e) = swarm.listen_on(listen_addr) {
            tracing::error!(error = %e, "RSU GossipSub listen failed");
            return;
        }

        let bootstrap_addr: libp2p::Multiaddr =
            format!("/memory/{BOOTSTRAP_PORT}").parse().expect("bootstrap addr");

        let dial_bootstrap = |swarm: &mut libp2p::Swarm<VanetBehaviour>| {
            let opts = DialOpts::unknown_peer_id()
                .address(bootstrap_addr.clone())
                .build();
            if let Err(e) = swarm.dial(opts) {
                tracing::warn!(error = %e, "RSU GossipSub bootstrap dial failed");
            }
        };
        dial_bootstrap(&mut swarm);

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
                    match event {
                        SwarmEvent::OutgoingConnectionError { error, .. } => {
                            tracing::debug!(error = %error, "RSU GossipSub bootstrap connection failed, retrying");
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            dial_bootstrap(&mut swarm);
                        }
                        SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(ev)) => {
                            tracing::trace!(?ev, "RSU GossipSub event");
                        }
                        _ => {}
                    }
                }
            }
        }
    });
}

/// Spawn a GossipSub heartbeat-receiving task for an OBU node.
///
/// Dials the bootstrap relay at `/memory/BOOTSTRAP_PORT` to join the shared
/// in-process GossipSub mesh.  No RSU address is needed — the bootstrap
/// relays heartbeats from all RSUs transparently, which is correct for mobile
/// OBUs that may be in range of different RSUs over time.
pub fn spawn_obu_gossipsub_task(handle_heartbeat: impl Fn(Vec<u8>) + Send + 'static) {
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

        let bootstrap_addr: libp2p::Multiaddr =
            format!("/memory/{BOOTSTRAP_PORT}").parse().expect("bootstrap addr");

        let dial = |swarm: &mut libp2p::Swarm<VanetBehaviour>| {
            let opts = DialOpts::unknown_peer_id()
                .address(bootstrap_addr.clone())
                .build();
            if let Err(e) = swarm.dial(opts) {
                tracing::warn!(error = %e, "OBU GossipSub bootstrap dial failed");
            }
        };
        dial(&mut swarm);

        loop {
            match swarm.select_next_some().await {
                SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(
                    libp2p::gossipsub::Event::Message { message, .. },
                )) => {
                    handle_heartbeat(message.data);
                }
                SwarmEvent::OutgoingConnectionError { error, .. } => {
                    tracing::debug!(error = %error, "OBU GossipSub connection failed, retrying");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    dial(&mut swarm);
                }
                _ => {}
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
