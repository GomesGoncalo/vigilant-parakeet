use crate::{dev::Device, messages::Message};
use anyhow::Result;
use std::sync::Arc;

pub enum ReplyType {
    Wire(Vec<Arc<[u8]>>),
    Tap(Vec<Arc<[u8]>>),
}

pub trait Node {
    fn handle_msg(&self, msg: &Message) -> Result<Option<Vec<ReplyType>>>;
    fn generate(&self, dev: Arc<Device>);
}
