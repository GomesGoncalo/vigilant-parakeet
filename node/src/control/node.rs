use crate::{
    dev::Device,
    messages::{Data, Message},
};
use anyhow::Result;
use mac_address::MacAddress;
use std::sync::Arc;

#[derive(Debug)]
pub enum ReplyType {
    Wire(Vec<Arc<[u8]>>),
    Tap(Vec<Arc<[u8]>>),
}

pub trait Node {
    fn get_mac(&self) -> MacAddress;
    fn tap_traffic(&self, msg: Arc<Data>) -> Result<Option<Vec<ReplyType>>>;
    fn handle_msg(&self, msg: Message) -> Result<Option<Vec<ReplyType>>>;
    fn generate(&self, dev: Arc<Device>);
    fn get_route_to(&self, mac_address: Option<MacAddress>) -> Option<MacAddress>;
}
