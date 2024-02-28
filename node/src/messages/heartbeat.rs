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
    pub fn new(source: MacAddress, now: Duration, id: u32) -> Self {
        Self {
            source,
            now,
            id,
            hops: 0,
        }
    }
}

impl TryFrom<&[u8]> for HeartBeat {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let now = u128::from_le_bytes(value[..16].try_into()?);
        let now = Duration::from_millis(u64::try_from(now)?);
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

impl TryFrom<&[Arc<[u8]>]> for HeartBeat {
    type Error = anyhow::Error;

    fn try_from(value: &[Arc<[u8]>]) -> Result<Self, Self::Error> {
        let now = u128::from_le_bytes(value[0][..].try_into()?);
        let now = Duration::from_millis(u64::try_from(now)?);
        let id = u32::from_le_bytes(value[1][..].try_into()?);
        let hops = u32::from_le_bytes(value[2][..].try_into()?);
        let source = MacAddress::new(value[3][..].try_into()?);
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
    pub sender: MacAddress,
    pub now: Duration,
    pub id: u32,
    pub hops: u32,
}

impl HeartBeatReply {
    pub fn new(source: MacAddress, sender: MacAddress, now: Duration, id: u32, hops: u32) -> Self {
        Self {
            source,
            sender,
            now,
            id,
            hops,
        }
    }

    pub fn from_sender(value: HeartBeat, sender: MacAddress) -> Self {
        Self {
            now: value.now,
            id: value.id,
            hops: value.hops,
            source: value.source,
            sender,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.now
    }
}

impl TryFrom<&[u8]> for HeartBeatReply {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let now = u128::from_le_bytes(value[..16].try_into()?);
        let now = Duration::from_millis(now.try_into()?);
        let id = u32::from_le_bytes(value[16..20].try_into()?);
        let hops = u32::from_le_bytes(value[20..24].try_into()?);
        let source = MacAddress::new(value[24..30].try_into()?);
        let sender = MacAddress::new(value[30..36].try_into()?);
        Ok(Self {
            source,
            sender,
            now,
            id,
            hops,
        })
    }
}

impl TryFrom<&[Arc<[u8]>]> for HeartBeatReply {
    type Error = anyhow::Error;

    fn try_from(value: &[Arc<[u8]>]) -> Result<Self, Self::Error> {
        let now = u128::from_le_bytes(value[0][..].try_into()?);
        let now = Duration::from_millis(now.try_into()?);
        let id = u32::from_le_bytes(value[1][..].try_into()?);
        let hops = u32::from_le_bytes(value[2][..].try_into()?);
        let source = MacAddress::new(value[3][..].try_into()?);
        let sender = MacAddress::new(value[4][..].try_into()?);
        Ok(Self {
            source,
            sender,
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
            value.sender.bytes().into(),
        ]
    }
}
