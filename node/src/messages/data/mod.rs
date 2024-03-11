use std::borrow::Cow;

use anyhow::bail;
use mac_address::MacAddress;

#[derive(Debug, Clone)]
pub struct ToUpstream<'a> {
    origin: Cow<'a, [u8]>,
    data: Cow<'a, [u8]>,
}

#[derive(Debug, Clone)]
pub struct ToDownstream<'a> {
    origin: Cow<'a, [u8]>,
    destination: Cow<'a, [u8]>,
    data: Cow<'a, [u8]>,
}

#[derive(Debug)]
pub enum Data<'a> {
    Downstream(ToDownstream<'a>),
    Upstream(ToUpstream<'a>),
}

impl<'a> ToUpstream<'a> {
    pub fn new(node: MacAddress, data: &'a [u8]) -> Self {
        Self {
            origin: Cow::Owned(node.bytes().to_vec()),
            data: Cow::Borrowed(data),
        }
    }

    pub fn data(&self) -> &Cow<'_, [u8]> {
        &self.data
    }

    pub fn source(&self) -> &Cow<'_, [u8]> {
        &self.origin
    }
}

impl<'a> ToDownstream<'a> {
    pub fn new(origin: &'a [u8], destination: MacAddress, data: &'a [u8]) -> Self {
        Self {
            origin: Cow::Borrowed(origin),
            destination: Cow::Owned(destination.bytes().to_vec()),
            data: Cow::Borrowed(data),
        }
    }

    pub fn data(&self) -> &Cow<'_, [u8]> {
        &self.data
    }

    pub fn source(&self) -> &Cow<'_, [u8]> {
        &self.origin
    }
    pub fn destination(&self) -> &Cow<'_, [u8]> {
        &self.destination
    }
}

impl<'a> TryFrom<&'a [u8]> for ToUpstream<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let (Some(data), Some(origin)) = (value.get(6..), value.get(..6)) else {
            bail!("cannot get members");
        };
        let origin = Cow::Borrowed(origin);
        let data = Cow::Borrowed(data);
        Ok(Self { origin, data })
    }
}

impl<'a> From<&ToUpstream<'a>> for Vec<Vec<u8>> {
    fn from(value: &ToUpstream<'a>) -> Self {
        vec![value.origin.to_vec(), value.data.to_vec()]
    }
}

impl<'a> TryFrom<&'a [u8]> for ToDownstream<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let (Some(data), Some(destination), Some(origin)) =
            (value.get(12..), value.get(6..12), value.get(..6))
        else {
            bail!("cannot get members");
        };
        let destination = Cow::Borrowed(destination);
        let origin = Cow::Borrowed(origin);
        let data = Cow::Borrowed(data);
        Ok(Self {
            origin,
            destination,
            data,
        })
    }
}

impl<'a> From<&ToDownstream<'a>> for Vec<Vec<u8>> {
    fn from(value: &ToDownstream<'a>) -> Self {
        vec![
            value.origin.to_vec(),
            value.destination.to_vec(),
            value.data.to_vec(),
        ]
    }
}

impl<'a> TryFrom<&'a [u8]> for Data<'a> {
    type Error = anyhow::Error;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let Some(next) = value.get(1..) else {
            bail!("could not get next");
        };

        match value.first() {
            Some(0u8) => Ok(Self::Upstream(next.try_into()?)),
            Some(1u8) => Ok(Self::Downstream(next.try_into()?)),
            _ => bail!("is not a valid packet type"),
        }
    }
}

impl<'a> From<&Data<'a>> for Vec<Vec<u8>> {
    fn from(value: &Data<'a>) -> Self {
        match value {
            Data::Upstream(c) => {
                let mut result = vec![vec![0u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
            Data::Downstream(c) => {
                let mut result = vec![vec![1u8]];
                let more: Vec<Vec<u8>> = c.into();
                result.extend(more);
                result
            }
        }
    }
}
