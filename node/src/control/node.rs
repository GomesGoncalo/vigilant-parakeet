use crate::{dev::Device, messages::Message};
use anyhow::Result;
use mac_address::MacAddress;
use std::{fmt::Display, sync::Arc, time::Duration};

pub enum ReplyType {
    Wire(Vec<Arc<[u8]>>),
    Tap(Vec<Arc<[u8]>>),
}

pub struct Route {
    pub hops: u32,
    pub mac: MacAddress,
    pub latency: Option<Duration>,
}

impl Display for Route {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Route {{ mac: {}, hops: {}, latency: {:?} }}",
            self.mac, self.hops, self.latency
        )
    }
}

pub trait Node {
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>>;
    fn generate(&self, dev: Arc<Device>);
    fn get_route_to(&self, mac_address: MacAddress) -> Option<MacAddress>;
}
