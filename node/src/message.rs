use std::sync::Arc;

use anyhow::{bail, Result};
use uuid::Uuid;

enum BufType {
    Received(Arc<[u8]>),
    Send(Vec<Arc<[u8]>>),
}

pub struct Message {
    buf: BufType,
    pub uuid: Uuid,
}

pub enum PacketType<'a> {
    Control,
    Data(&'a [u8]),
}

impl<'a> TryFrom<&'a [u8]> for PacketType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        Ok(match value[0] {
            0 => Self::Control,
            1 => Self::Data(&value[1..]),
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
            0 => Self::Control,
            1 => Self::Data(&value[1]),
            _ => bail!("invalid packet type"),
        })
    }
}

impl Message {
    pub fn from(&self) -> &[u8] {
        match &self.buf {
            BufType::Received(buf) => &buf[0..6],
            BufType::Send(buf) => &buf[0],
        }
    }
    pub fn to(&self) -> &[u8] {
        match &self.buf {
            BufType::Received(buf) => &buf[6..12],
            BufType::Send(buf) => &buf[1],
        }
    }
    pub fn next_layer(&self) -> Result<PacketType> {
        match &self.buf {
            BufType::Received(buf) => buf[14 + 16..].try_into(),
            BufType::Send(buf) => buf[4..].try_into(),
        }
    }

    pub fn new(from: [u8; 6], to: [u8; 6], uuidb: &Uuid, ptype: &PacketType) -> Self {
        let messages = vec![
            to.into(),
            from.into(),
            [0x30, 0x30].into(),
            uuidb.to_bytes_le().into(),
            {
                match *ptype {
                    PacketType::Control => [0],
                    PacketType::Data(..) => [1],
                }
            }
            .into(),
            {
                match *ptype {
                    PacketType::Control => todo!(),
                    PacketType::Data(buf) => buf,
                }
            }
            .into(),
        ];
        Self {
            buf: BufType::Send(messages),
            uuid: *uuidb,
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

        let uuid = Uuid::from_bytes_le(buf[14..14 + 16].try_into()?);

        Ok(Self {
            buf: BufType::Received(buf),
            uuid,
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
