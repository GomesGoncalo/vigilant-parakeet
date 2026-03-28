use std::borrow::Cow;

use mac_address::MacAddress;

/// Key exchange initiation message.
///
/// Sent by an OBU to begin a Diffie-Hellman key negotiation with a peer.
///
/// Wire format (42 bytes):
///   key_id (4 bytes) | public_key (32 bytes) | sender (6 bytes)
#[derive(Debug, Clone)]
pub struct KeyExchangeInit<'a> {
    key_id: Cow<'a, [u8]>,
    public_key: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
}

impl<'a> KeyExchangeInit<'a> {
    pub fn new(key_id: u32, public_key: [u8; 32], sender: MacAddress) -> Self {
        Self {
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            public_key: Cow::Owned(public_key.to_vec()),
            sender: Cow::Owned(sender.bytes().to_vec()),
        }
    }

    pub fn key_id(&self) -> u32 {
        u32::from_be_bytes(
            self.key_id
                .get(0..4)
                .expect("key_id must be 4 bytes")
                .try_into()
                .expect("convert key_id"),
        )
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
            .get(0..32)
            .expect("public_key must be 32 bytes")
            .try_into()
            .expect("convert public_key")
    }

    pub fn sender(&self) -> MacAddress {
        MacAddress::new(
            self.sender
                .get(0..6)
                .expect("sender must be 6 bytes")
                .try_into()
                .expect("convert sender"),
        )
    }
}

impl<'a> TryFrom<&'a [u8]> for KeyExchangeInit<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < 42 {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: 42,
                actual: value.len(),
            });
        }

        let key_id = value
            .get(0..4)
            .ok_or(crate::error::NodeError::BufferTooShort {
                expected: 4,
                actual: value.len(),
            })?;
        let public_key = value
            .get(4..36)
            .ok_or(crate::error::NodeError::BufferTooShort {
                expected: 36,
                actual: value.len(),
            })?;
        let sender = value
            .get(36..42)
            .ok_or(crate::error::NodeError::BufferTooShort {
                expected: 42,
                actual: value.len(),
            })?;

        Ok(Self {
            key_id: Cow::Borrowed(key_id),
            public_key: Cow::Borrowed(public_key),
            sender: Cow::Borrowed(sender),
        })
    }
}

impl<'a> From<&KeyExchangeInit<'a>> for Vec<u8> {
    fn from(value: &KeyExchangeInit<'a>) -> Self {
        let mut buf = Vec::with_capacity(42);
        buf.extend_from_slice(&value.key_id);
        buf.extend_from_slice(&value.public_key);
        buf.extend_from_slice(&value.sender);
        buf
    }
}

/// Key exchange reply message.
///
/// Sent by a peer in response to a KeyExchangeInit.
///
/// Wire format (42 bytes):
///   key_id (4 bytes) | public_key (32 bytes) | sender (6 bytes)
#[derive(Debug, Clone)]
pub struct KeyExchangeReply<'a> {
    key_id: Cow<'a, [u8]>,
    public_key: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
}

impl<'a> KeyExchangeReply<'a> {
    pub fn new(key_id: u32, public_key: [u8; 32], sender: MacAddress) -> Self {
        Self {
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            public_key: Cow::Owned(public_key.to_vec()),
            sender: Cow::Owned(sender.bytes().to_vec()),
        }
    }

    pub fn key_id(&self) -> u32 {
        u32::from_be_bytes(
            self.key_id
                .get(0..4)
                .expect("key_id must be 4 bytes")
                .try_into()
                .expect("convert key_id"),
        )
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public_key
            .get(0..32)
            .expect("public_key must be 32 bytes")
            .try_into()
            .expect("convert public_key")
    }

    pub fn sender(&self) -> MacAddress {
        MacAddress::new(
            self.sender
                .get(0..6)
                .expect("sender must be 6 bytes")
                .try_into()
                .expect("convert sender"),
        )
    }
}

impl<'a> TryFrom<&'a [u8]> for KeyExchangeReply<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < 42 {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: 42,
                actual: value.len(),
            });
        }

        let key_id = value
            .get(0..4)
            .ok_or(crate::error::NodeError::BufferTooShort {
                expected: 4,
                actual: value.len(),
            })?;
        let public_key = value
            .get(4..36)
            .ok_or(crate::error::NodeError::BufferTooShort {
                expected: 36,
                actual: value.len(),
            })?;
        let sender = value
            .get(36..42)
            .ok_or(crate::error::NodeError::BufferTooShort {
                expected: 42,
                actual: value.len(),
            })?;

        Ok(Self {
            key_id: Cow::Borrowed(key_id),
            public_key: Cow::Borrowed(public_key),
            sender: Cow::Borrowed(sender),
        })
    }
}

impl<'a> From<&KeyExchangeReply<'a>> for Vec<u8> {
    fn from(value: &KeyExchangeReply<'a>) -> Self {
        let mut buf = Vec::with_capacity(42);
        buf.extend_from_slice(&value.key_id);
        buf.extend_from_slice(&value.public_key);
        buf.extend_from_slice(&value.sender);
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_exchange_init_roundtrip() {
        let key_id = 42u32;
        let public_key = [7u8; 32];
        let sender: MacAddress = [1, 2, 3, 4, 5, 6].into();

        let init = KeyExchangeInit::new(key_id, public_key, sender);
        assert_eq!(init.key_id(), key_id);
        assert_eq!(init.public_key(), public_key);
        assert_eq!(init.sender(), sender);

        let bytes: Vec<u8> = (&init).into();
        assert_eq!(bytes.len(), 42);

        let parsed = KeyExchangeInit::try_from(&bytes[..]).expect("parse");
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
    }

    #[test]
    fn key_exchange_reply_roundtrip() {
        let key_id = 99u32;
        let public_key = [0xAB; 32];
        let sender: MacAddress = [10, 20, 30, 40, 50, 60].into();

        let reply = KeyExchangeReply::new(key_id, public_key, sender);
        assert_eq!(reply.key_id(), key_id);
        assert_eq!(reply.public_key(), public_key);
        assert_eq!(reply.sender(), sender);

        let bytes: Vec<u8> = (&reply).into();
        assert_eq!(bytes.len(), 42);

        let parsed = KeyExchangeReply::try_from(&bytes[..]).expect("parse");
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
    }

    #[test]
    fn key_exchange_init_too_short_fails() {
        let short = [0u8; 20];
        assert!(KeyExchangeInit::try_from(&short[..]).is_err());
    }

    #[test]
    fn key_exchange_reply_too_short_fails() {
        let short = [0u8; 30];
        assert!(KeyExchangeReply::try_from(&short[..]).is_err());
    }
}
