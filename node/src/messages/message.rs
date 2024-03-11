use std::borrow::Cow;

use anyhow::{bail, Result};
use mac_address::MacAddress;

use super::packet_type::PacketType;

#[derive(Debug)]
pub struct Message<'a> {
    from: Cow<'a, [u8]>,
    to: Cow<'a, [u8]>,
    next: PacketType<'a>,
}

impl<'a> Message<'a> {
    pub fn new(from: MacAddress, to: MacAddress, next: PacketType<'a>) -> Self {
        Self {
            from: Cow::Owned(from.bytes().to_vec()),
            to: Cow::Owned(to.bytes().to_vec()),
            next,
        }
    }

    pub fn from(&self) -> Result<MacAddress> {
        Self::get_mac_address(self.from.get(0..6))
    }

    pub fn to(&self) -> Result<MacAddress> {
        Self::get_mac_address(self.to.get(0..6))
    }

    fn get_mac_address(opt: Option<&[u8]>) -> Result<MacAddress> {
        let Some(opt) = opt else {
            bail!("no buffer");
        };

        let opt: [u8; 6] = opt.try_into()?;
        Ok(opt.into())
    }

    pub fn get_packet_type(&'a self) -> &PacketType<'a> {
        &self.next
    }
}

impl<'a> TryFrom<&'a [u8]> for Message<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.get(12..14) != Some(&[0x30, 0x30]) {
            bail!("not from this protocol");
        }

        let Some(from) = value.get(6..12) else {
            bail!("cannot get from");
        };

        let Some(to) = value.get(0..6) else {
            bail!("cannot get to");
        };

        let Some(next) = value.get(14..) else {
            bail!("cannot get packet type");
        };

        Ok(Self {
            from: Cow::Borrowed(from),
            to: Cow::Borrowed(to),
            next: next.try_into()?,
        })
    }
}

impl<'a> From<&Message<'a>> for Vec<Vec<u8>> {
    fn from(value: &Message<'a>) -> Self {
        let mut this = vec![
            value.to.clone().into_owned(),
            value.from.clone().into_owned(),
            vec![0x30, 0x30],
        ];
        let more: Vec<Vec<u8>> = (&value.next).into();
        this.extend(more);
        this
    }
}

#[cfg(test)]
mod tests {
    use crate::messages::message::Message;

    #[test]
    fn message_not_from_this_protocol_cannot_be_built() {
        let pkt = [0u8; 14];
        let msg = Message::try_from(&pkt[..]);
        assert!(msg.is_err());
    }
}
