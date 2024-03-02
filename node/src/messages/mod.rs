pub mod heartbeat;
pub use heartbeat::{HeartBeat, HeartBeatReply};

use anyhow::{bail, Result};
use mac_address::MacAddress;
use std::sync::Arc;

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
pub struct DownstreamData<'a> {
    pub source: MacAddress,
    pub data: &'a [u8],
}

impl<'a> DownstreamData<'a> {
    pub fn new(source: MacAddress, data: &'a [u8]) -> Self {
        Self { source, data }
    }
}

impl<'a> TryFrom<&'a [u8]> for DownstreamData<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let from: [u8; 6] = value[0..6].try_into()?;
        Ok(Self {
            source: from.into(),
            data: &value[6..],
        })
    }
}

impl<'a> TryFrom<&'a Arc<[u8]>> for DownstreamData<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a Arc<[u8]>) -> Result<Self, Self::Error> {
        let from: [u8; 6] = value[0..6].try_into()?;
        Ok(Self {
            source: from.into(),
            data: &value[6..],
        })
    }
}

impl<'a> TryFrom<&'a [Arc<[u8]>]> for DownstreamData<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [Arc<[u8]>]) -> Result<Self, Self::Error> {
        let from: &[u8; 6] = value[0][0..6].try_into()?;
        Ok(Self {
            source: (*from).into(),
            data: &value[1],
        })
    }
}

impl<'a> From<&DownstreamData<'a>> for Vec<Arc<[u8]>> {
    fn from(value: &DownstreamData) -> Self {
        vec![value.source.bytes().into(), value.data.into()]
    }
}

#[derive(Debug)]
pub struct UpstreamData<'a> {
    pub source: MacAddress,
    pub destination: MacAddress,
    pub data: &'a [u8],
}

impl<'a> UpstreamData<'a> {
    pub fn new(source: MacAddress, destination: MacAddress, data: &'a [u8]) -> Self {
        Self {
            source,
            destination,
            data,
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for UpstreamData<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let from: [u8; 6] = value[0..6].try_into()?;
        let to: [u8; 6] = value[6..12].try_into()?;
        Ok(Self {
            source: from.into(),
            destination: to.into(),
            data: &value[12..],
        })
    }
}

impl<'a> TryFrom<&'a Arc<[u8]>> for UpstreamData<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a Arc<[u8]>) -> Result<Self, Self::Error> {
        let from: [u8; 6] = value[0..6].try_into()?;
        let to: [u8; 6] = value[6..12].try_into()?;
        Ok(Self {
            source: from.into(),
            destination: to.into(),
            data: &value[12..],
        })
    }
}

impl<'a> TryFrom<&'a [Arc<[u8]>]> for UpstreamData<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [Arc<[u8]>]) -> Result<Self, Self::Error> {
        let from: &[u8; 6] = value[0][0..6].try_into()?;
        let to: &[u8; 6] = value[1][0..6].try_into()?;
        Ok(Self {
            source: (*from).into(),
            destination: (*to).into(),
            data: &value[2],
        })
    }
}

impl<'a> From<&UpstreamData<'a>> for Vec<Arc<[u8]>> {
    fn from(value: &UpstreamData) -> Self {
        vec![
            value.source.bytes().into(),
            value.destination.bytes().into(),
            value.data.into(),
        ]
    }
}

#[derive(Debug)]
pub enum Data<'a> {
    Downstream(Arc<DownstreamData<'a>>),
    Upstream(Arc<UpstreamData<'a>>),
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
    Data(Data<'a>),
}

impl<'a> TryFrom<&'a [u8]> for PacketType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(match value[0] {
            0 => Self::Control(value[1..].try_into()?),
            1 => Self::Data(Data::Downstream(Arc::new(value[1..].try_into()?))),
            2 => Self::Data(Data::Upstream(Arc::new(value[1..].try_into()?))),
            _ => bail!("invalid packet type"),
        })
    }
}

impl<'a> TryFrom<&'a [Arc<[u8]>]> for PacketType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [Arc<[u8]>]) -> Result<Self, Self::Error> {
        if value[0].len() != 1 {
            bail!("This is vectored the packet type should be a single value");
        }
        Ok(match value[0][0] {
            0 => Self::Control(value[1..].try_into()?),
            1 => Self::Data(Data::Downstream(Arc::new(value[1..].try_into()?))),
            2 => Self::Data(Data::Upstream(Arc::new(value[1..].try_into()?))),
            _ => bail!("invalid packet type"),
        })
    }
}

impl Message {
    pub fn from(&self) -> MacAddress {
        let buf: [u8; 6] = match &self.buf {
            BufType::Received(buf) => &buf[6..12],
            BufType::Send(buf) => &buf[1],
        }
        .try_into()
        .unwrap();
        buf.into()
    }
    pub fn to(&self) -> MacAddress {
        let buf: [u8; 6] = match &self.buf {
            BufType::Received(buf) => &buf[0..6],
            BufType::Send(buf) => &buf[0],
        }
        .try_into()
        .unwrap();
        buf.into()
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
                    PacketType::Data(Data::Downstream(..)) => [1],
                    PacketType::Data(Data::Upstream(..)) => [2],
                }
            }
            .into(),
        ];

        let more: Vec<Arc<[u8]>> = match ptype {
            PacketType::Data(Data::Downstream(buf)) => {
                let buf: &DownstreamData = buf;
                buf.into()
            }
            PacketType::Data(Data::Upstream(buf)) => {
                let buf: &UpstreamData = buf;
                buf.into()
            }
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

impl TryFrom<Vec<Arc<[u8]>>> for Message {
    type Error = anyhow::Error;

    fn try_from(buf: Vec<Arc<[u8]>>) -> Result<Self, Self::Error> {
        let mut it = buf.iter().flat_map(|inner| inner.iter()).skip(12);
        let Some(x1) = it.next() else {
            bail!("not enough");
        };
        let Some(x2) = it.next() else {
            bail!("not enough");
        };

        if (x1, x2) != (&0x30, &0x30) {
            bail!("not protocol")
        }

        Ok(Self {
            buf: BufType::Received(buf.iter().flat_map(|inner| inner.iter()).copied().collect()),
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
