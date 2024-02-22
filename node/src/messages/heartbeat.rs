use std::sync::Arc;
use std::time::Duration;

use mac_address::MacAddress;

#[derive(Debug, Clone)]
pub struct HeartBeat {
    pub source: MacAddress,
    pub now: Duration,
    pub id: u32,
    pub hops: u32,
}

impl HeartBeat {
    pub fn new(source: &MacAddress, since_boot: Duration, id: u32) -> Self {
        Self {
            source: source.clone(),
            now: since_boot,
            id,
            hops: 0,
        }
    }
}

impl TryFrom<&[u8]> for HeartBeat {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let now = u128::from_le_bytes(value[..16].try_into()?);
        let now = Duration::from_millis(now as u64);
        let id = u32::from_le_bytes(value[16..20].try_into()?);
        let hops = u32::from_le_bytes(value[20..24].try_into()?);
        let source = MacAddress::new(value[24..30].try_into()?);
        Ok(Self {
            source,
            now,
            id,
            hops,
        })
    }
}

impl From<&HeartBeat> for Vec<Arc<[u8]>> {
    fn from(value: &HeartBeat) -> Self {
        let now = value.now.as_millis();
        let hops = value.hops + 1;
        vec![
            now.to_le_bytes().into(),
            value.id.to_le_bytes().into(),
            hops.to_le_bytes().into(),
            value.source.bytes().into(),
        ]
    }
}

#[derive(Debug)]
pub struct HeartBeatReply {
    pub source: MacAddress,
    pub now: Duration,
    pub id: u32,
    pub hops: u32,
}

impl HeartBeatReply {
    pub fn new(source: &MacAddress, since_boot: Duration, id: u32, hops: u32) -> Self {
        Self {
            source: source.clone(),
            now: since_boot,
            id,
            hops,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.now
    }
}

impl TryFrom<&[u8]> for HeartBeatReply {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        // TODO: from_milliseconds takes u64 but as_millis is u128.
        // Take the safe approach. Here this method can fail & since
        // we are never putting more than u64 in this it's still
        // safe to get the u128 (the high bits will be 0s)
        let now = u128::from_le_bytes(value[..16].try_into()?);
        let now = Duration::from_millis(now.try_into()?);
        let id = u32::from_le_bytes(value[16..20].try_into()?);
        let hops = u32::from_le_bytes(value[20..24].try_into()?);
        let source = MacAddress::new(value[24..30].try_into()?);
        Ok(Self {
            source,
            now,
            id,
            hops,
        })
    }
}

impl From<&HeartBeatReply> for Vec<Arc<[u8]>> {
    fn from(value: &HeartBeatReply) -> Self {
        let now = value.now.as_millis();
        vec![
            now.to_le_bytes().into(),
            value.id.to_le_bytes().into(),
            value.hops.to_le_bytes().into(),
            value.source.bytes().into(),
        ]
    }
}

impl From<HeartBeat> for HeartBeatReply {
    fn from(value: HeartBeat) -> Self {
        Self {
            now: value.now,
            id: value.id,
            hops: value.hops,
            source: value.source,
        }
    }
}
