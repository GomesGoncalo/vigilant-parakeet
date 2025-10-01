use super::packet_type::PacketType;
use crate::error::NodeError;
use mac_address::MacAddress;
use std::borrow::Cow;

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

    pub fn from(&self) -> Result<MacAddress, NodeError> {
        Self::get_mac_address(self.from.get(0..6))
    }

    pub fn to(&self) -> Result<MacAddress, NodeError> {
        Self::get_mac_address(self.to.get(0..6))
    }

    fn get_mac_address(opt: Option<&[u8]>) -> Result<MacAddress, NodeError> {
        let opt = opt.ok_or(NodeError::InvalidMacAddress)?;
        let opt: [u8; 6] = opt.try_into().map_err(|_| NodeError::InvalidMacAddress)?;
        Ok(opt.into())
    }

    pub fn get_packet_type(&self) -> &PacketType<'a> {
        &self.next
    }
}

impl<'a> TryFrom<&'a [u8]> for Message<'a> {
    type Error = NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.get(12..14) != Some(&[0x30, 0x30]) {
            return Err(NodeError::InvalidProtocol);
        }

        let from = value.get(6..12).ok_or_else(|| {
            NodeError::BufferTooShort {
                expected: 12,
                actual: value.len(),
            }
        })?;

        let to = value.get(0..6).ok_or_else(|| {
            NodeError::BufferTooShort {
                expected: 6,
                actual: value.len(),
            }
        })?;

        let next = value.get(14..).ok_or_else(|| {
            NodeError::BufferTooShort {
                expected: 15,
                actual: value.len(),
            }
        })?;

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

    #[test]
    fn message_too_short_is_error() {
        // shorter than the minimum header length -> error
        let pkt = [0u8; 8];
        let msg = Message::try_from(&pkt[..]);
        assert!(msg.is_err());
    }

    #[test]
    fn message_with_no_packet_type_is_error() {
        // Build a slice that contains the protocol marker and from/to but no payload
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[1u8; 6]); // to
        pkt.extend_from_slice(&[2u8; 6]); // from
        pkt.extend_from_slice(&[0x30, 0x30]); // marker
                                              // no more bytes
        let msg = Message::try_from(&pkt[..]);
        assert!(msg.is_err());
    }
}
