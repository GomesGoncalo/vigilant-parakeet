use libp2p::{
    gossipsub::{self, IdentTopic, MessageAuthenticity, ValidationMode},
    identify,
    identity::Keypair,
    swarm::NetworkBehaviour,
};
use std::time::Duration;

/// GossipSub topic used to disseminate VANET heartbeat messages.
pub const HEARTBEAT_TOPIC: &str = "vanet/heartbeat/v1";

/// Combined libp2p network behaviour for VANET nodes.
///
/// - `gossipsub`: pub-sub mesh for heartbeat broadcast (RSU → all OBUs)
/// - `identify`: announces protocol version and observed addresses
#[derive(NetworkBehaviour)]
pub struct VanetBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub identify: identify::Behaviour,
}

impl VanetBehaviour {
    /// Construct the behaviour, subscribe to the given topics.
    pub fn new(
        keypair: &Keypair,
        topics: &[IdentTopic],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::build(keypair, topics, 3, 2, 6)
    }

    /// Construct the relay/bootstrap behaviour with a large mesh capacity.
    ///
    /// Used by the in-process bootstrap node so it can hold all RSU and OBU
    /// swarms in its mesh without pruning them.
    pub fn new_relay(
        keypair: &Keypair,
        topics: &[IdentTopic],
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Self::build(keypair, topics, 100, 1, 1000)
    }

    fn build(
        keypair: &Keypair,
        topics: &[IdentTopic],
        mesh_n: usize,
        mesh_n_low: usize,
        mesh_n_high: usize,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(Duration::from_millis(50))
            .validation_mode(ValidationMode::Strict)
            .mesh_n(mesh_n)
            .mesh_n_low(mesh_n_low)
            .mesh_n_high(mesh_n_high)
            .build()
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("gossipsub config error: {e}").into()
            })?;

        let mut gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            format!("gossipsub init error: {e}").into()
        })?;

        for topic in topics {
            gossipsub.subscribe(topic).map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("subscribe error: {e}").into()
                },
            )?;
        }

        let identify = identify::Behaviour::new(identify::Config::new(
            "/vanet/1.0.0".to_string(),
            keypair.public(),
        ));

        Ok(Self {
            gossipsub,
            identify,
        })
    }
}
