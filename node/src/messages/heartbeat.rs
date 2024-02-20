use std::sync::Arc;
use std::time::Duration;

#[derive(Debug)]
pub struct HeartBeat {
    now: Duration,
}

impl HeartBeat {
    pub fn new(since_boot: Duration) -> Self {
        Self { now: since_boot }
    }

    pub fn elapsed(&self) -> Duration {
        self.now
    }
}

impl TryFrom<&[u8]> for HeartBeat {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let now = u64::from_le_bytes(value.try_into()?);
        let now = Duration::from_millis(now);
        Ok(Self { now })
    }
}

impl From<&HeartBeat> for Vec<Arc<[u8]>> {
    fn from(value: &HeartBeat) -> Self {
        let value = value.now.as_millis();
        vec![(value as u64).to_le_bytes().into()]
    }
}

#[derive(Debug)]
pub struct HeartBeatReply {
    now: Duration,
}

impl HeartBeatReply {
    pub fn new(since_boot: Duration) -> Self {
        Self { now: since_boot }
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
        Ok(Self { now })
    }
}

impl From<&HeartBeatReply> for Vec<Arc<[u8]>> {
    fn from(value: &HeartBeatReply) -> Self {
        let now = value.now.as_millis();
        vec![now.to_le_bytes().into()]
    }
}
