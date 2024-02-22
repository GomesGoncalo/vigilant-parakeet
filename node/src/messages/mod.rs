pub mod heartbeat;
pub use heartbeat::{HeartBeat, HeartBeatReply};

use std::sync::Arc;

use anyhow::{bail, Result};

#[derive(Debug)]
pub enum ControlType {
    HeartBeat(HeartBeat),
    HeartBeatReply(HeartBeatReply),
}

impl TryFrom<&[u8]> for ControlType {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        Ok(match value[0] {
            0 => ControlType::HeartBeat(HeartBeat::try_from(&value[1..])?),
            1 => ControlType::HeartBeatReply(HeartBeatReply::try_from(&value[1..])?),
            _ => bail!("not supported"),
        })
    }
}

impl TryFrom<Arc<[u8]>> for ControlType {
    type Error = anyhow::Error;

    fn try_from(value: Arc<[u8]>) -> Result<Self, Self::Error> {
        Ok(match value[0] {
            0 => ControlType::HeartBeat(HeartBeat::try_from(&value[1..])?),
            1 => ControlType::HeartBeatReply(HeartBeatReply::try_from(&value[1..])?),
            _ => bail!("not supported"),
        })
    }
}

impl TryFrom<&[Arc<[u8]>]> for ControlType {
    type Error = anyhow::Error;

    fn try_from(value: &[Arc<[u8]>]) -> Result<Self, Self::Error> {
        Ok(match value[0][0] {
            0 => ControlType::HeartBeat(HeartBeat::try_from(&value[1..])?),
            1 => ControlType::HeartBeatReply(HeartBeatReply::try_from(&value[1..])?),
            _ => bail!("not supported"),
        })
    }
}

impl From<&ControlType> for Vec<Arc<[u8]>> {
    fn from(value: &ControlType) -> Self {
        match value {
            ControlType::HeartBeat(hb) => {
                let mut v = vec![vec![0u8].into()];
                v.extend(Into::<Vec<Arc<[u8]>>>::into(hb));
                v
            }
            ControlType::HeartBeatReply(hb) => {
                let mut v = vec![vec![1u8].into()];
                v.extend(Into::<Vec<Arc<[u8]>>>::into(hb));
                v
            }
        }
    }
}

#[derive(Debug)]
enum BufType {
    Received(Arc<[u8]>),
    Send(Vec<Arc<[u8]>>),
}

#[derive(Debug)]
pub struct Message {
    buf: BufType,
}

#[derive(Debug)]
pub enum PacketType<'a> {
    Control(ControlType),
    Data(&'a [u8]),
}

impl<'a> TryFrom<&'a [u8]> for PacketType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(match value[0] {
            0 => Self::Control(value[1..].try_into()?),
            1 => Self::Data(&value[1..]),
            _ => bail!("invalid packet type"),
        })
    }
}

impl<'a> TryFrom<&'a [Arc<[u8]>]> for PacketType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [Arc<[u8]>]) -> Result<Self, Self::Error> {
        eprintln!("{:?}", value);
        if value[0].len() != 1 {
            bail!("This is vectored the packet type should be a single value");
        }
        Ok(match value[0][0] {
            0 => Self::Control(value[1..].try_into()?),
            1 => Self::Data(&value[1]),
            _ => bail!("invalid packet type"),
        })
    }
}

impl Message {
    pub fn from(&self) -> &[u8] {
        match &self.buf {
            BufType::Received(buf) => &buf[6..12],
            BufType::Send(buf) => &buf[1],
        }
    }
    pub fn to(&self) -> &[u8] {
        match &self.buf {
            BufType::Received(buf) => &buf[0..6],
            BufType::Send(buf) => &buf[0],
        }
    }
    pub fn next_layer(&self) -> Result<PacketType> {
        match &self.buf {
            BufType::Received(buf) => buf[14..].try_into(),
            BufType::Send(buf) => buf[3..].try_into(),
        }
    }

    pub fn new(from: [u8; 6], to: [u8; 6], ptype: &PacketType) -> Self {
        let mut messages: Vec<Arc<[u8]>> = vec![
            to.into(),
            from.into(),
            [0x30, 0x30].into(),
            {
                match *ptype {
                    PacketType::Control(..) => [0],
                    PacketType::Data(..) => [1],
                }
            }
            .into(),
        ];

        let more: Vec<Arc<[u8]>> = match ptype {
            PacketType::Data(buf) => vec![(*buf).into()],
            PacketType::Control(buf) => buf.into(),
        };

        messages.extend(more);

        Self {
            buf: BufType::Send(messages),
        }
    }
}

impl TryFrom<Arc<[u8]>> for Message {
    type Error = anyhow::Error;

    fn try_from(buf: Arc<[u8]>) -> Result<Self, Self::Error> {
        if buf.len() < 14 {
            bail!("too small")
        }

        if buf[12..14] != [0x30, 0x30] {
            bail!("not protocol")
        }

        Ok(Self {
            buf: BufType::Received(buf),
        })
    }
}

impl From<Message> for Vec<Arc<[u8]>> {
    fn from(value: Message) -> Self {
        match value.buf {
            BufType::Received(buf) => vec![buf],
            BufType::Send(buf) => buf,
        }
    }
}
