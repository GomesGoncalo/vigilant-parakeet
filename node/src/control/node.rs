use crate::{dev::Device, messages::Message};
use anyhow::Result;
use mac_address::MacAddress;
use std::sync::Arc;

pub enum ReplyType {
    Wire(Vec<Arc<[u8]>>),
    Tap(Vec<Arc<[u8]>>),
}

pub trait Node {
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>>;
    fn generate(&self, dev: Arc<Device>);
    fn get_route_to(&self, mac_address: &MacAddress) -> Option<MacAddress>;
}
