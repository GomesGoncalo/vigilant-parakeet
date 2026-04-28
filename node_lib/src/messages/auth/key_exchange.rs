use std::borrow::Cow;

use mac_address::MacAddress;

use crate::crypto::{DhGroup, SigningAlgorithm};

// ── Wire format sizes ─────────────────────────────────────────────────
//
// Unsigned base format:
//   algo_id (1) | key_id (4) | key_material_len (2 BE) | key_material (var) | sender (6)
//
// Signed extension (appended after base):
//   sig_algo_id (1) | signing_pubkey_len (2 BE) | signing_pubkey (var)
//   | signature_len (2 BE) | signature (var)

/// Minimum bytes needed to read the base header before key_material.
/// algo_id(1) + key_id(4) + key_material_len(2) = 7 bytes.
pub const KE_HEADER_LEN: usize = 7;
/// Sender MAC appended after key_material.
pub const KE_SENDER_LEN: usize = 6;
/// Minimum bytes needed to parse a complete unsigned message (header + 1 byte key + sender).
pub const KE_MIN_LEN: usize = KE_HEADER_LEN + 1 + KE_SENDER_LEN;

// Legacy constants (X25519 + Ed25519 sizes) kept for documentation/tests.
/// X25519 key material length (public key).
pub const X25519_KEY_LEN: usize = 32;
/// Ed25519 verifying key length.
pub const ED25519_VK_LEN: usize = 32;
/// Ed25519 signature length.
pub const ED25519_SIG_LEN: usize = 64;

// ── KeyExchangeInit ───────────────────────────────────────────────────

/// Key exchange initiation message.
///
/// Sent by an OBU to begin a key negotiation with the server.
///
/// Wire format — unsigned base:
///   `algo_id (1) | key_id (4) | key_material_len (2 BE) | key_material (var) | sender (6)`
///
/// Wire format — signed extension appended after base:
///   `sig_algo_id (1) | spk_len (2 BE) | signing_pubkey (var) | sig_len (2 BE) | signature (var)`
///
/// For X25519: `key_material` = 32-byte DH public key.
/// For ML-KEM-768: `key_material` = 1184-byte encapsulation key.
#[derive(Debug, Clone)]
pub struct KeyExchangeInit<'a> {
    /// Wire-format algorithm byte (derived from DhGroup).
    algo_id: u8,
    key_id: Cow<'a, [u8]>,
    key_material: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
    /// Wire-format sig algorithm byte (derived from SigningAlgorithm), if signed.
    sig_algo_id: Option<u8>,
    signing_pubkey: Option<Cow<'a, [u8]>>,
    signature: Option<Cow<'a, [u8]>>,
}

impl<'a> KeyExchangeInit<'a> {
    /// Create an unsigned key exchange initiation.
    pub fn new_unsigned(
        dh_group: DhGroup,
        key_id: u32,
        key_material: Vec<u8>,
        sender: MacAddress,
    ) -> Self {
        Self {
            algo_id: dh_group.wire_id(),
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            key_material: Cow::Owned(key_material),
            sender: Cow::Owned(sender.bytes().to_vec()),
            sig_algo_id: None,
            signing_pubkey: None,
            signature: None,
        }
    }

    /// Create a signed key exchange initiation.
    pub fn new_signed(
        dh_group: DhGroup,
        key_id: u32,
        key_material: Vec<u8>,
        sender: MacAddress,
        sig_algo: SigningAlgorithm,
        signing_pubkey: Vec<u8>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            algo_id: dh_group.wire_id(),
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            key_material: Cow::Owned(key_material),
            sender: Cow::Owned(sender.bytes().to_vec()),
            sig_algo_id: Some(sig_algo.wire_id()),
            signing_pubkey: Some(Cow::Owned(signing_pubkey)),
            signature: Some(Cow::Owned(signature)),
        }
    }

    pub fn clone_into_owned(&self) -> KeyExchangeInit<'static> {
        KeyExchangeInit {
            algo_id: self.algo_id,
            key_id: Cow::Owned(self.key_id.to_vec()),
            key_material: Cow::Owned(self.key_material.to_vec()),
            sender: Cow::Owned(self.sender.to_vec()),
            sig_algo_id: self.sig_algo_id,
            signing_pubkey: self.signing_pubkey.as_ref().map(|b| Cow::Owned(b.to_vec())),
            signature: self.signature.as_ref().map(|b| Cow::Owned(b.to_vec())),
        }
    }

    /// Returns the DH group for this message, or `None` for unrecognised wire IDs.
    pub fn dh_group(&self) -> Option<DhGroup> {
        DhGroup::from_wire_id(self.algo_id)
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

    pub fn key_material(&self) -> &[u8] {
        &self.key_material
    }

    /// Convenience: return the 32-byte X25519 public key. Panics if not X25519 / wrong size.
    pub fn public_key(&self) -> [u8; 32] {
        self.key_material
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

    /// Returns the signing algorithm, or `None` if unsigned or unrecognised wire ID.
    pub fn signing_algorithm(&self) -> Option<SigningAlgorithm> {
        self.sig_algo_id.and_then(SigningAlgorithm::from_wire_id)
    }

    pub fn signing_pubkey(&self) -> Option<&[u8]> {
        self.signing_pubkey.as_deref()
    }

    pub fn signature(&self) -> Option<&[u8]> {
        self.signature.as_deref()
    }

    pub fn is_signed(&self) -> bool {
        self.sig_algo_id.is_some() && self.signing_pubkey.is_some() && self.signature.is_some()
    }

    /// Return the base payload bytes to be signed (or verified against).
    ///
    /// Covers: algo_id | key_id | key_material_len | key_material | sender.
    pub fn base_payload(&self) -> Vec<u8> {
        let km_len = self.key_material.len() as u16;
        let mut buf = Vec::with_capacity(KE_HEADER_LEN + self.key_material.len() + KE_SENDER_LEN);
        buf.push(self.algo_id);
        buf.extend_from_slice(&self.key_id);
        buf.extend_from_slice(&km_len.to_be_bytes());
        buf.extend_from_slice(&self.key_material);
        buf.extend_from_slice(&self.sender);
        buf
    }

    /// Wire size in bytes without allocating.
    pub fn wire_size(&self) -> usize {
        // algo_id(1) + key_id(4) + km_len(2) + key_material + sender(6)
        let base = 1 + self.key_id.len() + 2 + self.key_material.len() + self.sender.len();
        match (&self.sig_algo_id, &self.signing_pubkey, &self.signature) {
            (Some(_), Some(spk), Some(sig)) => {
                // sig_algo_id(1) + spk_len(2) + spk + sig_len(2) + sig
                base + 1 + 2 + spk.len() + 2 + sig.len()
            }
            _ => base,
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for KeyExchangeInit<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < KE_MIN_LEN {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: KE_MIN_LEN,
                actual: value.len(),
            });
        }

        let algo_id = value[0];
        let key_id = &value[1..5];
        let km_len = u16::from_be_bytes([value[5], value[6]]) as usize;

        let km_end = KE_HEADER_LEN + km_len;
        let sender_end = km_end + KE_SENDER_LEN;
        if value.len() < sender_end {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: sender_end,
                actual: value.len(),
            });
        }

        let key_material = &value[KE_HEADER_LEN..km_end];
        let sender = &value[km_end..sender_end];

        // Parse optional signed extension.
        let (sig_algo_id, signing_pubkey, signature) = if value.len() > sender_end {
            let ext = &value[sender_end..];
            if ext.len() < 5 {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeInit signed extension too short".to_string(),
                ));
            }
            let sig_algo = ext[0];
            let signing_algo = SigningAlgorithm::from_wire_id(sig_algo).ok_or_else(|| {
                crate::error::NodeError::InvalidMessage(format!(
                    "KeyExchangeInit unknown sig_algo_id {sig_algo}"
                ))
            })?;
            let spk_len = u16::from_be_bytes([ext[1], ext[2]]) as usize;
            let (expected_spk_len, expected_sig_len) = match signing_algo {
                SigningAlgorithm::Ed25519 => (
                    crate::crypto::ED25519_VK_LEN,
                    crate::crypto::ED25519_SIG_LEN,
                ),
                SigningAlgorithm::MlDsa65 => (
                    crate::crypto::ML_DSA_65_VK_LEN,
                    crate::crypto::ML_DSA_65_SIG_LEN,
                ),
            };
            if spk_len != expected_spk_len {
                return Err(crate::error::NodeError::InvalidMessage(format!(
                    "KeyExchangeInit spk_len {spk_len} does not match sig_algo {sig_algo} (expected {expected_spk_len})"
                )));
            }
            let spk_end = 3 + spk_len;
            if ext.len() < spk_end + 2 {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeInit signed extension truncated before sig_len".to_string(),
                ));
            }
            let spk = &ext[3..spk_end];
            let sig_len = u16::from_be_bytes([ext[spk_end], ext[spk_end + 1]]) as usize;
            if sig_len != expected_sig_len {
                return Err(crate::error::NodeError::InvalidMessage(format!(
                    "KeyExchangeInit sig_len {sig_len} does not match sig_algo {sig_algo} (expected {expected_sig_len})"
                )));
            }
            let sig_end = spk_end + 2 + sig_len;
            if ext.len() < sig_end {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeInit signed extension truncated before end of signature"
                        .to_string(),
                ));
            }
            if ext.len() != sig_end {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeInit has trailing bytes after signature".to_string(),
                ));
            }
            let sig = &ext[spk_end + 2..sig_end];
            (
                Some(sig_algo),
                Some(Cow::Borrowed(spk)),
                Some(Cow::Borrowed(sig)),
            )
        } else {
            (None, None, None)
        };

        Ok(Self {
            algo_id,
            key_id: Cow::Borrowed(key_id),
            key_material: Cow::Borrowed(key_material),
            sender: Cow::Borrowed(sender),
            sig_algo_id,
            signing_pubkey,
            signature,
        })
    }
}

impl<'a> From<&KeyExchangeInit<'a>> for Vec<u8> {
    fn from(value: &KeyExchangeInit<'a>) -> Self {
        let km_len = value.key_material.len() as u16;
        let mut buf = Vec::new();
        buf.push(value.algo_id);
        buf.extend_from_slice(&value.key_id);
        buf.extend_from_slice(&km_len.to_be_bytes());
        buf.extend_from_slice(&value.key_material);
        buf.extend_from_slice(&value.sender);
        if let (Some(sig_algo), Some(spk), Some(sig)) =
            (value.sig_algo_id, &value.signing_pubkey, &value.signature)
        {
            buf.push(sig_algo);
            let spk_len = spk.len() as u16;
            buf.extend_from_slice(&spk_len.to_be_bytes());
            buf.extend_from_slice(spk);
            let sig_len = sig.len() as u16;
            buf.extend_from_slice(&sig_len.to_be_bytes());
            buf.extend_from_slice(sig);
        }
        buf
    }
}

// ── KeyExchangeReply ──────────────────────────────────────────────────

/// Key exchange reply message.
///
/// Sent by the server in response to a `KeyExchangeInit`.
///
/// For X25519: `key_material` = 32-byte DH public key.
/// For ML-KEM-768: `key_material` = 1088-byte KEM ciphertext.
#[derive(Debug, Clone)]
pub struct KeyExchangeReply<'a> {
    algo_id: u8,
    key_id: Cow<'a, [u8]>,
    key_material: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
    sig_algo_id: Option<u8>,
    signing_pubkey: Option<Cow<'a, [u8]>>,
    signature: Option<Cow<'a, [u8]>>,
}

impl<'a> KeyExchangeReply<'a> {
    /// Create an unsigned key exchange reply.
    pub fn new_unsigned(
        dh_group: DhGroup,
        key_id: u32,
        key_material: Vec<u8>,
        sender: MacAddress,
    ) -> Self {
        Self {
            algo_id: dh_group.wire_id(),
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            key_material: Cow::Owned(key_material),
            sender: Cow::Owned(sender.bytes().to_vec()),
            sig_algo_id: None,
            signing_pubkey: None,
            signature: None,
        }
    }

    /// Create a signed key exchange reply.
    pub fn new_signed(
        dh_group: DhGroup,
        key_id: u32,
        key_material: Vec<u8>,
        sender: MacAddress,
        sig_algo: SigningAlgorithm,
        signing_pubkey: Vec<u8>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            algo_id: dh_group.wire_id(),
            key_id: Cow::Owned(key_id.to_be_bytes().to_vec()),
            key_material: Cow::Owned(key_material),
            sender: Cow::Owned(sender.bytes().to_vec()),
            sig_algo_id: Some(sig_algo.wire_id()),
            signing_pubkey: Some(Cow::Owned(signing_pubkey)),
            signature: Some(Cow::Owned(signature)),
        }
    }

    pub fn clone_into_owned(&self) -> KeyExchangeReply<'static> {
        KeyExchangeReply {
            algo_id: self.algo_id,
            key_id: Cow::Owned(self.key_id.to_vec()),
            key_material: Cow::Owned(self.key_material.to_vec()),
            sender: Cow::Owned(self.sender.to_vec()),
            sig_algo_id: self.sig_algo_id,
            signing_pubkey: self.signing_pubkey.as_ref().map(|b| Cow::Owned(b.to_vec())),
            signature: self.signature.as_ref().map(|b| Cow::Owned(b.to_vec())),
        }
    }

    /// Returns the DH group for this message, or `None` for unrecognised wire IDs.
    pub fn dh_group(&self) -> Option<DhGroup> {
        DhGroup::from_wire_id(self.algo_id)
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

    pub fn key_material(&self) -> &[u8] {
        &self.key_material
    }

    /// Convenience: return the 32-byte X25519 public key. Panics if not X25519 / wrong size.
    pub fn public_key(&self) -> [u8; 32] {
        self.key_material
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

    /// Returns the signing algorithm, or `None` if unsigned or unrecognised wire ID.
    pub fn signing_algorithm(&self) -> Option<SigningAlgorithm> {
        self.sig_algo_id.and_then(SigningAlgorithm::from_wire_id)
    }

    pub fn signing_pubkey(&self) -> Option<&[u8]> {
        self.signing_pubkey.as_deref()
    }

    pub fn signature(&self) -> Option<&[u8]> {
        self.signature.as_deref()
    }

    pub fn is_signed(&self) -> bool {
        self.sig_algo_id.is_some() && self.signing_pubkey.is_some() && self.signature.is_some()
    }

    /// Return the base payload bytes to be signed (or verified against).
    pub fn base_payload(&self) -> Vec<u8> {
        let km_len = self.key_material.len() as u16;
        let mut buf = Vec::with_capacity(KE_HEADER_LEN + self.key_material.len() + KE_SENDER_LEN);
        buf.push(self.algo_id);
        buf.extend_from_slice(&self.key_id);
        buf.extend_from_slice(&km_len.to_be_bytes());
        buf.extend_from_slice(&self.key_material);
        buf.extend_from_slice(&self.sender);
        buf
    }

    /// Wire size in bytes without allocating.
    pub fn wire_size(&self) -> usize {
        let base = 1 + self.key_id.len() + 2 + self.key_material.len() + self.sender.len();
        match (&self.sig_algo_id, &self.signing_pubkey, &self.signature) {
            (Some(_), Some(spk), Some(sig)) => base + 1 + 2 + spk.len() + 2 + sig.len(),
            _ => base,
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for KeyExchangeReply<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < KE_MIN_LEN {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: KE_MIN_LEN,
                actual: value.len(),
            });
        }

        let algo_id = value[0];
        let key_id = &value[1..5];
        let km_len = u16::from_be_bytes([value[5], value[6]]) as usize;

        let km_end = KE_HEADER_LEN + km_len;
        let sender_end = km_end + KE_SENDER_LEN;
        if value.len() < sender_end {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: sender_end,
                actual: value.len(),
            });
        }

        let key_material = &value[KE_HEADER_LEN..km_end];
        let sender = &value[km_end..sender_end];

        let (sig_algo_id, signing_pubkey, signature) = if value.len() > sender_end {
            let ext = &value[sender_end..];
            if ext.len() < 5 {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeReply signed extension too short".to_string(),
                ));
            }
            let sig_algo = ext[0];
            let signing_algo = SigningAlgorithm::from_wire_id(sig_algo).ok_or_else(|| {
                crate::error::NodeError::InvalidMessage(format!(
                    "KeyExchangeReply unknown sig_algo_id {sig_algo}"
                ))
            })?;
            let spk_len = u16::from_be_bytes([ext[1], ext[2]]) as usize;
            let (expected_spk_len, expected_sig_len) = match signing_algo {
                SigningAlgorithm::Ed25519 => (
                    crate::crypto::ED25519_VK_LEN,
                    crate::crypto::ED25519_SIG_LEN,
                ),
                SigningAlgorithm::MlDsa65 => (
                    crate::crypto::ML_DSA_65_VK_LEN,
                    crate::crypto::ML_DSA_65_SIG_LEN,
                ),
            };
            if spk_len != expected_spk_len {
                return Err(crate::error::NodeError::InvalidMessage(format!(
                    "KeyExchangeReply spk_len {spk_len} does not match sig_algo {sig_algo} (expected {expected_spk_len})"
                )));
            }
            let spk_end = 3 + spk_len;
            if ext.len() < spk_end + 2 {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeReply signed extension truncated before sig_len".to_string(),
                ));
            }
            let spk = &ext[3..spk_end];
            let sig_len = u16::from_be_bytes([ext[spk_end], ext[spk_end + 1]]) as usize;
            if sig_len != expected_sig_len {
                return Err(crate::error::NodeError::InvalidMessage(format!(
                    "KeyExchangeReply sig_len {sig_len} does not match sig_algo {sig_algo} (expected {expected_sig_len})"
                )));
            }
            let sig_end = spk_end + 2 + sig_len;
            if ext.len() < sig_end {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeReply signed extension truncated before end of signature"
                        .to_string(),
                ));
            }
            if ext.len() != sig_end {
                return Err(crate::error::NodeError::InvalidMessage(
                    "KeyExchangeReply has trailing bytes after signature".to_string(),
                ));
            }
            let sig = &ext[spk_end + 2..sig_end];
            (
                Some(sig_algo),
                Some(Cow::Borrowed(spk)),
                Some(Cow::Borrowed(sig)),
            )
        } else {
            (None, None, None)
        };

        Ok(Self {
            algo_id,
            key_id: Cow::Borrowed(key_id),
            key_material: Cow::Borrowed(key_material),
            sender: Cow::Borrowed(sender),
            sig_algo_id,
            signing_pubkey,
            signature,
        })
    }
}

impl<'a> From<&KeyExchangeReply<'a>> for Vec<u8> {
    fn from(value: &KeyExchangeReply<'a>) -> Self {
        let km_len = value.key_material.len() as u16;
        let mut buf = Vec::new();
        buf.push(value.algo_id);
        buf.extend_from_slice(&value.key_id);
        buf.extend_from_slice(&km_len.to_be_bytes());
        buf.extend_from_slice(&value.key_material);
        buf.extend_from_slice(&value.sender);
        if let (Some(sig_algo), Some(spk), Some(sig)) =
            (value.sig_algo_id, &value.signing_pubkey, &value.signature)
        {
            buf.push(sig_algo);
            let spk_len = spk.len() as u16;
            buf.extend_from_slice(&spk_len.to_be_bytes());
            buf.extend_from_slice(spk);
            let sig_len = sig.len() as u16;
            buf.extend_from_slice(&sig_len.to_be_bytes());
            buf.extend_from_slice(sig);
        }
        buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x25519_unsigned_len() -> usize {
        // algo_id(1) + key_id(4) + km_len(2) + key(32) + sender(6)
        1 + 4 + 2 + 32 + 6
    }

    fn x25519_signed_len() -> usize {
        // base + sig_algo_id(1) + spk_len(2) + spk(32) + sig_len(2) + sig(64)
        x25519_unsigned_len() + 1 + 2 + 32 + 2 + 64
    }

    #[test]
    fn key_exchange_init_roundtrip() {
        let key_id = 42u32;
        let public_key = [7u8; 32];
        let sender: MacAddress = [1, 2, 3, 4, 5, 6].into();

        let init =
            KeyExchangeInit::new_unsigned(DhGroup::X25519, key_id, public_key.to_vec(), sender);
        assert_eq!(init.key_id(), key_id);
        assert_eq!(init.public_key(), public_key);
        assert_eq!(init.sender(), sender);
        assert!(!init.is_signed());
        assert_eq!(init.dh_group(), Some(DhGroup::X25519));

        let bytes: Vec<u8> = (&init).into();
        assert_eq!(bytes.len(), x25519_unsigned_len());

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

        let reply =
            KeyExchangeReply::new_unsigned(DhGroup::X25519, key_id, public_key.to_vec(), sender);
        assert_eq!(reply.key_id(), key_id);
        assert_eq!(reply.public_key(), public_key);
        assert_eq!(reply.sender(), sender);
        assert!(!reply.is_signed());

        let bytes: Vec<u8> = (&reply).into();
        assert_eq!(bytes.len(), x25519_unsigned_len());

        let parsed = KeyExchangeReply::try_from(&bytes[..]).expect("parse");
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert!(!parsed.is_signed());
    }

    #[test]
    fn key_exchange_init_too_short_fails() {
        let short = [0u8; 5];
        assert!(KeyExchangeInit::try_from(&short[..]).is_err());
    }

    #[test]
    fn key_exchange_reply_too_short_fails() {
        let short = [0u8; 5];
        assert!(KeyExchangeReply::try_from(&short[..]).is_err());
    }

    #[test]
    fn signed_init_roundtrip() {
        let key_id = 7u32;
        let public_key = [0x11u8; 32];
        let sender: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let signing_pubkey = [0x22u8; 32];
        let signature = [0x33u8; 64];

        let init = KeyExchangeInit::new_signed(
            DhGroup::X25519,
            key_id,
            public_key.to_vec(),
            sender,
            SigningAlgorithm::Ed25519,
            signing_pubkey.to_vec(),
            signature.to_vec(),
        );
        assert!(init.is_signed());
        assert_eq!(init.signing_pubkey(), Some(signing_pubkey.as_ref()));
        assert_eq!(init.signature(), Some(signature.as_ref()));
        assert_eq!(init.signing_algorithm(), Some(SigningAlgorithm::Ed25519));

        let bytes: Vec<u8> = (&init).into();
        assert_eq!(bytes.len(), x25519_signed_len());

        let parsed = KeyExchangeInit::try_from(&bytes[..]).expect("parse signed");
        assert!(parsed.is_signed());
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert_eq!(parsed.signing_pubkey(), Some(signing_pubkey.as_ref()));
        assert_eq!(parsed.signature(), Some(signature.as_ref()));
    }

    #[test]
    fn signed_reply_roundtrip() {
        let key_id = 99u32;
        let public_key = [0x55u8; 32];
        let sender: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let signing_pubkey = [0x66u8; 32];
        let signature = [0x77u8; 64];

        let reply = KeyExchangeReply::new_signed(
            DhGroup::X25519,
            key_id,
            public_key.to_vec(),
            sender,
            SigningAlgorithm::Ed25519,
            signing_pubkey.to_vec(),
            signature.to_vec(),
        );
        assert!(reply.is_signed());

        let bytes: Vec<u8> = (&reply).into();
        assert_eq!(bytes.len(), x25519_signed_len());

        let parsed = KeyExchangeReply::try_from(&bytes[..]).expect("parse signed");
        assert!(parsed.is_signed());
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.public_key(), public_key);
        assert_eq!(parsed.sender(), sender);
        assert_eq!(parsed.signing_pubkey(), Some(signing_pubkey.as_ref()));
        assert_eq!(parsed.signature(), Some(signature.as_ref()));
    }

    #[test]
    fn base_payload_covers_algo_key_sender() {
        let key_id = 1u32;
        let public_key = [0xAAu8; 32];
        let sender: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let init =
            KeyExchangeInit::new_unsigned(DhGroup::X25519, key_id, public_key.to_vec(), sender);
        let base = init.base_payload();
        // algo_id(1) + key_id(4) + km_len(2) + key(32) + sender(6)
        assert_eq!(base.len(), 1 + 4 + 2 + 32 + 6);
        assert_eq!(base[0], DhGroup::X25519.wire_id());
        assert_eq!(&base[1..5], &1u32.to_be_bytes());
        assert_eq!(&base[7..39], &[0xAAu8; 32]);
        assert_eq!(&base[39..45], &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn ml_kem_768_init_roundtrip() {
        let key_id = 5u32;
        let encap_key = [0xBBu8; crate::crypto::ML_KEM_768_EK_LEN];
        let sender: MacAddress = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66].into();

        let init =
            KeyExchangeInit::new_unsigned(DhGroup::MlKem768, key_id, encap_key.to_vec(), sender);
        assert_eq!(init.dh_group(), Some(DhGroup::MlKem768));
        assert_eq!(init.key_id(), key_id);
        assert_eq!(init.key_material(), &encap_key);
        assert!(!init.is_signed());

        let bytes: Vec<u8> = (&init).into();
        // algo_id(1) + key_id(4) + km_len(2) + encap_key(1184) + sender(6)
        assert_eq!(bytes.len(), 1 + 4 + 2 + 1184 + 6);

        let parsed = KeyExchangeInit::try_from(&bytes[..]).expect("parse ml-kem init");
        assert_eq!(parsed.dh_group(), Some(DhGroup::MlKem768));
        assert_eq!(parsed.key_id(), key_id);
        assert_eq!(parsed.key_material(), &encap_key);
        assert_eq!(parsed.sender(), sender);
    }

    #[test]
    fn ml_kem_768_reply_roundtrip() {
        let key_id = 6u32;
        let ciphertext = [0xCCu8; crate::crypto::ML_KEM_768_CT_LEN];
        let sender: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();

        let reply =
            KeyExchangeReply::new_unsigned(DhGroup::MlKem768, key_id, ciphertext.to_vec(), sender);
        assert_eq!(reply.dh_group(), Some(DhGroup::MlKem768));
        assert_eq!(reply.key_material(), &ciphertext);

        let bytes: Vec<u8> = (&reply).into();
        // algo_id(1) + key_id(4) + km_len(2) + ct(1088) + sender(6)
        assert_eq!(bytes.len(), 1 + 4 + 2 + 1088 + 6);

        let parsed = KeyExchangeReply::try_from(&bytes[..]).expect("parse ml-kem reply");
        assert_eq!(parsed.key_material(), &ciphertext);
        assert_eq!(parsed.sender(), sender);
    }

    #[test]
    fn clone_into_owned_preserves_data() {
        let init = KeyExchangeInit::new_unsigned(
            DhGroup::X25519,
            42,
            [1u8; 32].to_vec(),
            [1, 2, 3, 4, 5, 6].into(),
        );
        let bytes: Vec<u8> = (&init).into();
        let parsed = KeyExchangeInit::try_from(&bytes[..]).expect("parse");
        let owned = parsed.clone_into_owned();
        assert_eq!(owned.key_id(), 42);
        assert_eq!(owned.public_key(), [1u8; 32]);
    }
}
