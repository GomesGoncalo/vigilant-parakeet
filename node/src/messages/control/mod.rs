pub mod heartbeat;

use anyhow::bail;
use heartbeat::{Heartbeat, HeartbeatReply};

use self::heartbeat::HeartbeatAck;

#[derive(Debug)]
pub enum Control<'a> {
    Heartbeat(Heartbeat<'a>),
    HeartbeatReply(HeartbeatReply<'a>),
    HeartbeatAck(HeartbeatAck<'a>),
}

impl<'a> TryFrom<&'a [u8]> for Control<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let Some(next) = value.get(1..) else {
            bail!("could not get next");
        };

        match value.first() {
            Some(0u8) => Ok(Self::Heartbeat(next.try_into()?)),
            Some(1u8) => Ok(Self::HeartbeatReply(next.try_into()?)),
            Some(2u8) => Ok(Self::HeartbeatAck(next.try_into()?)),
            _ => bail!("is not a valid packet type"),
        }
    }
}

impl<'a> From<&Control<'a>> for Vec<Vec<u8>> {
    fn from(value: &Control<'a>) -> Self {
        match value {
            Control::Heartbeat(c) => {
                let mut result = vec![vec![0u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
            Control::HeartbeatReply(c) => {
                let mut result = vec![vec![1u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
            Control::HeartbeatAck(c) => {
                let mut result = vec![vec![2u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
        }
    }
}
