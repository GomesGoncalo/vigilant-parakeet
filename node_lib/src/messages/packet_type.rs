use super::{auth::Auth, control::Control, data::Data};
use crate::error::NodeError;

#[derive(Debug)]
pub enum PacketType<'a> {
    Control(Control<'a>),
    Data(Data<'a>),
    Auth(Auth<'a>),
}

impl<'a> PacketType<'a> {
    /// Wire size of this packet type in bytes (includes the 1-byte type tag).
    pub fn wire_size(&self) -> usize {
        1 + match self {
            PacketType::Control(c) => 1 + c.wire_size(),
            PacketType::Data(d) => 1 + d.wire_size(),
            PacketType::Auth(a) => 1 + a.wire_size(),
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for PacketType<'a> {
    type Error = NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let next = value.get(1..).ok_or_else(|| NodeError::BufferTooShort {
            expected: 2,
            actual: value.len(),
        })?;

        match value.first() {
            Some(0u8) => Ok(Self::Control(next.try_into()?)),
            Some(1u8) => Ok(Self::Data(next.try_into()?)),
            Some(2u8) => Ok(Self::Auth(next.try_into()?)),
            _ => Err(NodeError::ParseError(
                "Invalid packet type identifier".to_string(),
            )),
        }
    }
}

impl<'a> From<&PacketType<'a>> for Vec<u8> {
    fn from(value: &PacketType<'a>) -> Self {
        let mut buf = Vec::with_capacity(32);
        match value {
            PacketType::Control(c) => {
                buf.push(0u8);
                let control_bytes: Vec<u8> = c.into();
                buf.extend_from_slice(&control_bytes);
            }
            PacketType::Data(d) => {
                buf.push(1u8);
                let data_bytes: Vec<u8> = d.into();
                buf.extend_from_slice(&data_bytes);
            }
            PacketType::Auth(a) => {
                buf.push(2u8);
                let auth_bytes: Vec<u8> = a.into();
                buf.extend_from_slice(&auth_bytes);
            }
        }
        buf
    }
}

// Keep backwards compatibility
impl<'a> From<&PacketType<'a>> for Vec<Vec<u8>> {
    fn from(value: &PacketType<'a>) -> Self {
        match value {
            PacketType::Control(c) => {
                let mut result = vec![vec![0u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
            PacketType::Data(d) => {
                let mut result = vec![vec![1u8]];
                let more: Vec<Vec<u8>> = d.into();
                result.extend(more);
                result
            }
            PacketType::Auth(a) => {
                let mut result = vec![vec![2u8]];
                let auth_bytes: Vec<u8> = a.into();
                result.push(auth_bytes);
                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PacketType;

    #[test]
    fn packet_type_invalid_first_byte_is_error() {
        let pkt = [3u8];
        let res = PacketType::try_from(&pkt[..]);
        assert!(res.is_err());
    }

    #[test]
    fn packet_type_too_short_is_error() {
        let pkt: [u8; 1] = [0u8];
        let res = PacketType::try_from(&pkt[..]);
        assert!(res.is_err());
    }

    #[test]
    fn packet_type_roundtrip_control() {
        // construct a control packet type directly and ensure conversion includes leading byte
        use crate::messages::control::{heartbeat::Heartbeat, Control};
        use mac_address::MacAddress;
        use std::time::Duration;

        let hb = Heartbeat::new(Duration::default(), 0, MacAddress::new([0u8; 6]));
        let ctrl = Control::Heartbeat(hb);
        let pt = PacketType::Control(ctrl);
        let v: Vec<Vec<u8>> = (&pt).into();
        assert_eq!(v[0], vec![0u8]);
    }

    #[test]
    fn packet_type_roundtrip_data() {
        // construct a data packet type directly and ensure conversion includes leading byte
        use crate::messages::data::Data;
        use mac_address::MacAddress;

        let data = Data::Upstream(crate::messages::data::ToUpstream::new(
            MacAddress::new([0u8; 6]),
            &[],
        ));
        let pt = PacketType::Data(data);
        let v: Vec<Vec<u8>> = (&pt).into();
        assert_eq!(v[0], vec![1u8]);
    }
}
