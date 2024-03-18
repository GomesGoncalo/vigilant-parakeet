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
