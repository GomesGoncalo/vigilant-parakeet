use mac_address::MacAddress;
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub source: MacAddress,
    pub duration: Duration,
}

impl SessionRequest {
    pub fn new(source: MacAddress, duration: Duration) -> Self {
        Self { source, duration }
    }
}

impl TryFrom<&[u8]> for SessionRequest {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let source = MacAddress::new(value[0..6].try_into()?);
        let duration = u64::from_le_bytes(value[6..14].try_into()?);
        let duration = Duration::from_secs(u64::try_from(duration)?);
        Ok(Self { source, duration })
    }
}

impl TryFrom<&[Arc<[u8]>]> for SessionRequest {
    type Error = anyhow::Error;

    fn try_from(value: &[Arc<[u8]>]) -> Result<Self, Self::Error> {
        let source = MacAddress::new(value[0][..].try_into()?);
        let duration = u64::from_le_bytes(value[1][..].try_into()?);
        let duration = Duration::from_secs(u64::try_from(duration)?);
        Ok(Self { source, duration })
    }
}

impl From<&SessionRequest> for Vec<Arc<[u8]>> {
    fn from(value: &SessionRequest) -> Self {
        vec![
            value.source.bytes().into(),
            value.duration.as_secs().to_le_bytes().into(),
        ]
    }
}

#[derive(Debug)]
pub struct SessionResponse {
    pub source: MacAddress,
    pub duration: Duration,
}

impl SessionResponse {
    pub fn new(source: MacAddress, duration: Duration) -> Self {
        Self { source, duration }
    }
}

impl TryFrom<&[u8]> for SessionResponse {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let source = MacAddress::new(value[0..6].try_into()?);
        let duration = u64::from_le_bytes(value[6..14].try_into()?);
        let duration = Duration::from_secs(u64::try_from(duration)?);
        Ok(Self { source, duration })
    }
}

impl TryFrom<&[Arc<[u8]>]> for SessionResponse {
    type Error = anyhow::Error;

    fn try_from(value: &[Arc<[u8]>]) -> Result<Self, Self::Error> {
        let source = MacAddress::new(value[0][..].try_into()?);
        let duration = u64::from_le_bytes(value[1][..].try_into()?);
        let duration = Duration::from_secs(u64::try_from(duration)?);
        Ok(Self { source, duration })
    }
}

impl From<&SessionResponse> for Vec<Arc<[u8]>> {
    fn from(value: &SessionResponse) -> Self {
        vec![
            value.source.bytes().into(),
            value.duration.as_secs().to_le_bytes().into(),
        ]
    }
}
