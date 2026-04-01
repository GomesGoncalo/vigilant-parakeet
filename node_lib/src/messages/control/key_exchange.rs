use std::borrow::Cow;

use mac_address::MacAddress;

/// Base wire length for an unsigned key exchange message (bytes).
pub const KE_BASE_LEN: usize = 42;

/// Wire length of the signature extension: 32-byte Ed25519 verifying key +
/// 64-byte signature over the 42-byte base payload.
pub const KE_SIG_EXT_LEN: usize = 96;

/// Total wire length for a *signed* key exchange message.
pub const KE_SIGNED_LEN: usize = KE_BASE_LEN + KE_SIG_EXT_LEN; // 138 bytes

/// Key exchange initiation message.
///
/// Sent by an OBU to begin a Diffie-Hellman key negotiation with a peer.
///
/// Unsigned wire format (42 bytes):
///   key_id (4 bytes) | public_key (32 bytes) | sender (6 bytes)
///
/// Signed wire format (138 bytes):
///   key_id (4 bytes) | public_key (32 bytes) | sender (6 bytes)
///   | signing_pubkey (32 bytes) | signature (64 bytes)
///
/// The Ed25519 signature covers the first 42 bytes (the base payload).
#[derive(Debug, Clone)]
pub struct KeyExchangeInit<'a> {
    key_id: Cow<'a, [u8]>,
    public_key: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
    /// Optional 32-byte Ed25519 verifying key.
    signing_pubkey: Option<Cow<'a, [u8]>>,
    /// Optional 64-byte Ed25519 signature over the first 42 bytes.
    signature: Option<Cow<'a, [u8]>>,
}

impl<'a> KeyExchangeInit<'a> {
    /// Create an unsigned message.
    pub fn new(key_id: u32, public_key: [u8; 32], sender: MacAddress) -> Self {
        Self {
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            public_key: Cow::Owned(public_key.to_vec()),
            sender: Cow::Owned(sender.bytes().to_vec()),
            signing_pubkey: None,
            signature: None,
        }
    }

    /// Create a signed message.
    ///
    /// `signing_pubkey` is the 32-byte Ed25519 verifying key.
    /// `signature` is the 64-byte signature over the 42-byte base payload.
    pub fn new_signed(
        key_id: u32,
        public_key: [u8; 32],
        sender: MacAddress,
        signing_pubkey: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        Self {
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            public_key: Cow::Owned(public_key.to_vec()),
            sender: Cow::Owned(sender.bytes().to_vec()),
            signing_pubkey: Some(Cow::Owned(signing_pubkey.to_vec())),
            signature: Some(Cow::Owned(signature.to_vec())),
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

    /// Return the 32-byte Ed25519 verifying key, if present.
    pub fn signing_pubkey(&self) -> Option<[u8; 32]> {
        self.signing_pubkey.as_ref().map(|b| {
            b.get(0..32)
                .expect("signing_pubkey must be 32 bytes")
                .try_into()
                .expect("convert signing_pubkey")
        })
    }

    /// Return the 64-byte Ed25519 signature, if present.
    pub fn signature(&self) -> Option<[u8; 64]> {
        self.signature.as_ref().map(|b| {
            b.get(0..64)
                .expect("signature must be 64 bytes")
                .try_into()
                .expect("convert signature")
        })
    }

    /// Return whether this message carries a signature.
    pub fn is_signed(&self) -> bool {
        self.signing_pubkey.is_some()
    }

    /// Return the 42-byte base payload that was (or should be) signed.
    pub fn base_payload(&self) -> [u8; 42] {
        let mut buf = [0u8; 42];
        buf[..4].copy_from_slice(&self.key_id);
        buf[4..36].copy_from_slice(&self.public_key);
        buf[36..42].copy_from_slice(&self.sender);
        buf
    }
}

impl<'a> TryFrom<&'a [u8]> for KeyExchangeInit<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < KE_BASE_LEN {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: KE_BASE_LEN,
                actual: value.len(),
            });
        }

        let key_id = &value[0..4];
        let public_key = &value[4..36];
        let sender = &value[36..42];

        let (signing_pubkey, signature) = if value.len() >= KE_SIGNED_LEN {
            let spk = &value[42..74];
            let sig = &value[74..138];
            (Some(Cow::Borrowed(spk)), Some(Cow::Borrowed(sig)))
        } else {
            (None, None)
        };

        Ok(Self {
            key_id: Cow::Borrowed(key_id),
            public_key: Cow::Borrowed(public_key),
            sender: Cow::Borrowed(sender),
            signing_pubkey,
            signature,
        })
    }
}

impl<'a> From<&KeyExchangeInit<'a>> for Vec<u8> {
    fn from(value: &KeyExchangeInit<'a>) -> Self {
        let signed = value.signing_pubkey.is_some() && value.signature.is_some();
        let capacity = if signed { KE_SIGNED_LEN } else { KE_BASE_LEN };
        let mut buf = Vec::with_capacity(capacity);
        buf.extend_from_slice(&value.key_id);
        buf.extend_from_slice(&value.public_key);
        buf.extend_from_slice(&value.sender);
        if signed {
            buf.extend_from_slice(value.signing_pubkey.as_ref().unwrap());
            buf.extend_from_slice(value.signature.as_ref().unwrap());
        }
        buf
    }
}

/// Key exchange reply message.
///
/// Sent by a peer in response to a KeyExchangeInit.
///
/// Unsigned wire format (42 bytes):
///   key_id (4 bytes) | public_key (32 bytes) | sender (6 bytes)
///
/// Signed wire format (138 bytes):
///   key_id (4 bytes) | public_key (32 bytes) | sender (6 bytes)
///   | signing_pubkey (32 bytes) | signature (64 bytes)
///
/// The Ed25519 signature covers the first 42 bytes (the base payload).
#[derive(Debug, Clone)]
pub struct KeyExchangeReply<'a> {
    key_id: Cow<'a, [u8]>,
    public_key: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
    /// Optional 32-byte Ed25519 verifying key.
    signing_pubkey: Option<Cow<'a, [u8]>>,
    /// Optional 64-byte Ed25519 signature over the first 42 bytes.
    signature: Option<Cow<'a, [u8]>>,
}

impl<'a> KeyExchangeReply<'a> {
    /// Create an unsigned message.
    pub fn new(key_id: u32, public_key: [u8; 32], sender: MacAddress) -> Self {
        Self {
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            public_key: Cow::Owned(public_key.to_vec()),
            sender: Cow::Owned(sender.bytes().to_vec()),
            signing_pubkey: None,
            signature: None,
        }
    }

    /// Create a signed message.
    pub fn new_signed(
        key_id: u32,
        public_key: [u8; 32],
        sender: MacAddress,
        signing_pubkey: [u8; 32],
        signature: [u8; 64],
    ) -> Self {
        Self {
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            public_key: Cow::Owned(public_key.to_vec()),
            sender: Cow::Owned(sender.bytes().to_vec()),
            signing_pubkey: Some(Cow::Owned(signing_pubkey.to_vec())),
            signature: Some(Cow::Owned(signature.to_vec())),
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

    /// Return the 32-byte Ed25519 verifying key, if present.
    pub fn signing_pubkey(&self) -> Option<[u8; 32]> {
        self.signing_pubkey.as_ref().map(|b| {
            b.get(0..32)
                .expect("signing_pubkey must be 32 bytes")
                .try_into()
                .expect("convert signing_pubkey")
        })
    }

    /// Return the 64-byte Ed25519 signature, if present.
    pub fn signature(&self) -> Option<[u8; 64]> {
        self.signature.as_ref().map(|b| {
            b.get(0..64)
                .expect("signature must be 64 bytes")
                .try_into()
                .expect("convert signature")
        })
    }

    /// Return whether this message carries a signature.
    pub fn is_signed(&self) -> bool {
        self.signing_pubkey.is_some()
    }

    /// Return the 42-byte base payload that was (or should be) signed.
    pub fn base_payload(&self) -> [u8; 42] {
        let mut buf = [0u8; 42];
        buf[..4].copy_from_slice(&self.key_id);
        buf[4..36].copy_from_slice(&self.public_key);
        buf[36..42].copy_from_slice(&self.sender);
        buf
    }
}

impl<'a> TryFrom<&'a [u8]> for KeyExchangeReply<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < KE_BASE_LEN {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: KE_BASE_LEN,
                actual: value.len(),
            });
        }

        let key_id = &value[0..4];
        let public_key = &value[4..36];
        let sender = &value[36..42];

        let (signing_pubkey, signature) = if value.len() >= KE_SIGNED_LEN {
            let spk = &value[42..74];
            let sig = &value[74..138];
            (Some(Cow::Borrowed(spk)), Some(Cow::Borrowed(sig)))
        } else {
            (None, None)
        };

        Ok(Self {
            key_id: Cow::Borrowed(key_id),
            public_key: Cow::Borrowed(public_key),
            sender: Cow::Borrowed(sender),
            signing_pubkey,
            signature,
        })
    }
}

impl<'a> From<&KeyExchangeReply<'a>> for Vec<u8> {
    fn from(value: &KeyExchangeReply<'a>) -> Self {
        let signed = value.signing_pubkey.is_some() && value.signature.is_some();
        let capacity = if signed { KE_SIGNED_LEN } else { KE_BASE_LEN };
        let mut buf = Vec::with_capacity(capacity);
        buf.extend_from_slice(&value.key_id);
        buf.extend_from_slice(&value.public_key);
        buf.extend_from_slice(&value.sender);
        if signed {
            buf.extend_from_slice(value.signing_pubkey.as_ref().unwrap());
            buf.extend_from_slice(value.signature.as_ref().unwrap());
        }
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
        assert!(!init.is_signed());

        let bytes: Vec<u8> = (&init).into();
        assert_eq!(bytes.len(), KE_BASE_LEN);

        let parsed = KeyExchangeInit::try_from(&bytes[..]).expect("parse");
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert!(!parsed.is_signed());
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
        assert!(!reply.is_signed());

        let bytes: Vec<u8> = (&reply).into();
        assert_eq!(bytes.len(), KE_BASE_LEN);

        let parsed = KeyExchangeReply::try_from(&bytes[..]).expect("parse");
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert!(!parsed.is_signed());
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

    #[test]
    fn signed_init_roundtrip() {
        let key_id = 7u32;
        let public_key = [0x11u8; 32];
        let sender: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let signing_pubkey = [0x22u8; 32];
        let signature = [0x33u8; 64];

        let init =
            KeyExchangeInit::new_signed(key_id, public_key, sender, signing_pubkey, signature);
        assert!(init.is_signed());
        assert_eq!(init.signing_pubkey(), Some(signing_pubkey));
        assert_eq!(init.signature(), Some(signature));

        let bytes: Vec<u8> = (&init).into();
        assert_eq!(bytes.len(), KE_SIGNED_LEN);

        let parsed = KeyExchangeInit::try_from(&bytes[..]).expect("parse signed");
        assert!(parsed.is_signed());
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert_eq!(parsed.signing_pubkey(), Some(signing_pubkey));
        assert_eq!(parsed.signature(), Some(signature));
    }

    #[test]
    fn signed_reply_roundtrip() {
        let key_id = 99u32;
        let public_key = [0x55u8; 32];
        let sender: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let signing_pubkey = [0x66u8; 32];
        let signature = [0x77u8; 64];

        let reply =
            KeyExchangeReply::new_signed(key_id, public_key, sender, signing_pubkey, signature);
        assert!(reply.is_signed());

        let bytes: Vec<u8> = (&reply).into();
        assert_eq!(bytes.len(), KE_SIGNED_LEN);

        let parsed = KeyExchangeReply::try_from(&bytes[..]).expect("parse signed");
        assert!(parsed.is_signed());
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert_eq!(parsed.signing_pubkey(), Some(signing_pubkey));
        assert_eq!(parsed.signature(), Some(signature));
    }

    #[test]
    fn base_payload_is_first_42_bytes() {
        let key_id = 1u32;
        let public_key = [0xAAu8; 32];
        let sender: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let init = KeyExchangeInit::new(key_id, public_key, sender);
        let base = init.base_payload();
        assert_eq!(&base[..4], &1u32.to_be_bytes());
        assert_eq!(&base[4..36], &[0xAAu8; 32]);
        assert_eq!(&base[36..42], &[1, 2, 3, 4, 5, 6]);
    }
}
