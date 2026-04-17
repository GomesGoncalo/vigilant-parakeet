//! Integration test: two VANET nodes exchange heartbeat bytes over GossipSub
//! using libp2p's in-memory transport (no Device or root required).

use libp2p::{
    core::{transport::MemoryTransport, upgrade::Version},
    futures::StreamExt,
    gossipsub::IdentTopic,
    identity::Keypair,
    noise,
    swarm::{Config as SwarmConfig, SwarmEvent},
    yamux, SwarmBuilder, Transport,
};
use libp2p_vanet::behaviour::{VanetBehaviour, VanetBehaviourEvent, HEARTBEAT_TOPIC};
use std::time::Duration;
use tokio::time::timeout;

fn make_swarm(keypair: Keypair) -> libp2p::Swarm<VanetBehaviour> {
    let topic = IdentTopic::new(HEARTBEAT_TOPIC);
    SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_other_transport(|key| {
            let noise = noise::Config::new(key).expect("noise config");
            MemoryTransport::default()
                .upgrade(Version::V1Lazy)
                .authenticate(noise)
                .multiplex(yamux::Config::default())
                .boxed()
        })
        .expect("transport")
        .with_behaviour(|key| VanetBehaviour::new(key, &[topic]))
        .expect("behaviour")
        .with_swarm_config(|c: SwarmConfig| c.with_idle_connection_timeout(Duration::from_secs(10)))
        .build()
}

#[tokio::test]
async fn gossipsub_heartbeat_delivery() {
    let kp_rsu = Keypair::generate_ed25519();
    let kp_obu = Keypair::generate_ed25519();

    let mut rsu_swarm = make_swarm(kp_rsu);
    let mut obu_swarm = make_swarm(kp_obu);

    // RSU listens on a memory address.
    rsu_swarm
        .listen_on("/memory/1".parse().expect("addr"))
        .expect("listen");

    // Give the RSU swarm a tick to register the listener.
    loop {
        match rsu_swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { .. } => break,
            _ => {}
        }
    }

    let rsu_addr = rsu_swarm
        .listeners()
        .next()
        .cloned()
        .expect("RSU should have a listen address");

    let _rsu_peer_id = *rsu_swarm.local_peer_id();

    // OBU dials the RSU.
    obu_swarm.dial(rsu_addr.clone()).expect("dial");

    // Drive both swarms until GossipSub peers have subscribed to each other's topics.
    // This happens after connection + identify + gossipsub subscription exchange.
    let subscribed = timeout(Duration::from_secs(10), async {
        let mut rsu_subscribed = false;
        let mut obu_subscribed = false;
        loop {
            tokio::select! {
                event = rsu_swarm.select_next_some() => {
                    if let SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(
                        libp2p::gossipsub::Event::Subscribed { .. }
                    )) = event {
                        rsu_subscribed = true;
                    }
                    if rsu_subscribed && obu_subscribed { break; }
                }
                event = obu_swarm.select_next_some() => {
                    if let SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(
                        libp2p::gossipsub::Event::Subscribed { .. }
                    )) = event {
                        obu_subscribed = true;
                    }
                    if rsu_subscribed && obu_subscribed { break; }
                }
            }
        }
    })
    .await;
    assert!(
        subscribed.is_ok(),
        "GossipSub subscription exchange timed out"
    );

    // Simulate a heartbeat payload (30 bytes, matches VANET Heartbeat wire size).
    let heartbeat_bytes: Vec<u8> = vec![0xAB; 30];
    let topic = IdentTopic::new(HEARTBEAT_TOPIC);

    // RSU publishes the heartbeat into the mesh.
    rsu_swarm
        .behaviour_mut()
        .gossipsub
        .publish(topic.clone(), heartbeat_bytes.clone())
        .expect("publish");

    // Drive both swarms until OBU receives the message.
    let received = timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = rsu_swarm.select_next_some() => {}
                event = obu_swarm.select_next_some() => {
                    if let SwarmEvent::Behaviour(VanetBehaviourEvent::Gossipsub(
                        libp2p::gossipsub::Event::Message { message, .. }
                    )) = event {
                        return message.data;
                    }
                }
            }
        }
    })
    .await
    .expect("OBU should receive heartbeat within timeout");

    assert_eq!(
        received, heartbeat_bytes,
        "received payload must match published heartbeat"
    );
}
