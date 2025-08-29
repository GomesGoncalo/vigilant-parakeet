use anyhow::bail;
use mac_address::MacAddress;
use std::borrow::Cow;

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

#[cfg(test)]
mod tests {
    use super::{Data, ToDownstream, ToUpstream};
    use mac_address::MacAddress;

    #[test]
    fn to_upstream_parse_and_roundtrip() {
        let mac: MacAddress = [7u8; 6].into();
        let payload = [9u8, 8u8];
        let tu = ToUpstream::new(mac, &payload);
        let v: Vec<Vec<u8>> = (&tu).into();
        assert_eq!(v[0], mac.bytes().to_vec());
        assert_eq!(v[1], payload.to_vec());

        let mut full = vec![vec![0u8]];
        full.extend(v.clone());
        let flat: Vec<u8> = full.iter().flat_map(|x| x.iter()).copied().collect();
        let parsed = Data::try_from(&flat[..]).expect("parse upstream");
        match parsed {
            Data::Upstream(u) => {
                assert_eq!(u.source().to_vec(), mac.bytes().to_vec());
                assert_eq!(u.data().to_vec(), payload.to_vec());
            }
            _ => panic!("expected upstream"),
        }
    }

    #[test]
    fn to_downstream_parse_and_roundtrip() {
        let origin = [5u8; 6];
        let dest: MacAddress = [6u8; 6].into();
        let payload = [1u8, 2u8, 3u8];
        let td = ToDownstream::new(&origin, dest, &payload);
        let v: Vec<Vec<u8>> = (&td).into();
        assert_eq!(v[0], origin.to_vec());
        assert_eq!(v[1], dest.bytes().to_vec());
        assert_eq!(v[2], payload.to_vec());

        let mut full = vec![vec![1u8]];
        full.extend(v.clone());
        let flat: Vec<u8> = full.iter().flat_map(|x| x.iter()).copied().collect();
        let parsed = Data::try_from(&flat[..]).expect("parse downstream");
        match parsed {
            Data::Downstream(d) => {
                assert_eq!(d.source().to_vec(), origin.to_vec());
                assert_eq!(d.destination().to_vec(), dest.bytes().to_vec());
                assert_eq!(d.data().to_vec(), payload.to_vec());
            }
            _ => panic!("expected downstream"),
        }
    }

    #[test]
    fn data_invalid_first_byte_is_error() {
        let pkt = vec![2u8, 0, 1, 2];
        assert!(Data::try_from(&pkt[..]).is_err());
    }
}
