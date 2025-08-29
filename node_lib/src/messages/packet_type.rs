use super::{control::Control, data::Data};
use anyhow::bail;

#[derive(Debug)]
pub enum PacketType<'a> {
    Control(Control<'a>),
    Data(Data<'a>),
}

impl<'a> TryFrom<&'a [u8]> for PacketType<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let Some(next) = value.get(1..) else {
            bail!("could not get next");
        };

        match value.first() {
            Some(0u8) => Ok(Self::Control(next.try_into()?)),
            Some(1u8) => Ok(Self::Data(next.try_into()?)),
            _ => bail!("is not a valid packet type"),
        }
    }
}

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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PacketType;

    #[test]
    fn packet_type_invalid_first_byte_is_error() {
        let pkt = [2u8];
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
    use crate::messages::control::{Control, heartbeat::Heartbeat};
    use mac_address::MacAddress;
    use std::time::Duration;

    let hb = Heartbeat::new(Duration::default(), 0, MacAddress::new([0u8;6]));
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

    let data = Data::Upstream(crate::messages::data::ToUpstream::new(MacAddress::new([0u8;6]), &[]));
    let pt = PacketType::Data(data);
    let v: Vec<Vec<u8>> = (&pt).into();
    assert_eq!(v[0], vec![1u8]);
    }
}
