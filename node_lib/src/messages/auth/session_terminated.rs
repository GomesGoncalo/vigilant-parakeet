use mac_address::MacAddress;
use std::borrow::Cow;

use crate::crypto::SigningAlgorithm;

/// A `SessionTerminated` auth message sent by the server (via RSU relay) to
/// notify an OBU that its session has been revoked by the server administrator.
///
/// The target OBU must clear its DH session key and immediately re-initiate a
/// new key exchange.  Intermediate OBUs forward this message toward the target
/// using the embedded `target` MAC.
///
/// Binary layout (inside the Auth payload, after the type byte):
/// ```text
/// Unsigned:  [TARGET_OBU_MAC 6B]
/// Signed:    [TARGET_OBU_MAC 6B][TIMESTAMP_SECS 8B][NONCE 8B]
///            [SIG_ALGO_ID 1B][SIG_LEN 2B BE][SIGNATURE var]
/// ```
///
/// The signed payload (what the server signs) is:
/// ```text
/// [AUTH_TYPE_BYTE=0x02][TARGET_OBU_MAC 6B][TIMESTAMP_SECS 8B][NONCE 8B]
/// ```
///
/// **Replay prevention**: Two complementary mechanisms:
/// - **Timestamp**: OBU rejects messages where `|now − timestamp| > VALIDITY_SECS`.
///   This makes captured messages expire automatically.
/// - **Nonce**: OBU tracks recently-seen nonces in a time-bounded cache (entries
///   older than `VALIDITY_SECS` are pruned). This prevents replay within the
///   validity window. Because the cache is time-bounded (not count-bounded),
///   eviction never opens a replay window.
pub const VALIDITY_SECS: u64 = 60;
/// Clock skew tolerance: accept messages up to this many seconds in the future.
pub const CLOCK_SKEW_TOLERANCE_SECS: u64 = 5;

/// Wire type byte prepended to the signed payload; matches Auth::SessionTerminated sub-ID.
const SIGNED_PAYLOAD_TYPE_BYTE: u8 = 0x02;

#[derive(Debug, Clone)]
pub struct SessionTerminated<'a> {
    target: Cow<'a, [u8]>,
    /// Unix timestamp (seconds) when the server issued the revocation.
    timestamp_secs: Option<u64>,
    /// 8-byte server-generated random nonce.
    nonce: Option<[u8; 8]>,
    /// Wire-format signature algorithm byte, if signed.
    sig_algo_id: Option<u8>,
    /// Raw signature bytes from the server.
    signature: Option<Cow<'a, [u8]>>,
}

impl<'a> SessionTerminated<'a> {
    /// Create an unsigned message (no replay protection).
    pub fn new(target: MacAddress) -> Self {
        Self {
            target: Cow::Owned(target.bytes().to_vec()),
            timestamp_secs: None,
            nonce: None,
            sig_algo_id: None,
            signature: None,
        }
    }

    /// Create a signed message with a timestamp and nonce for replay prevention.
    pub fn new_signed(
        target: MacAddress,
        timestamp_secs: u64,
        nonce: [u8; 8],
        sig_algo: SigningAlgorithm,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            target: Cow::Owned(target.bytes().to_vec()),
            timestamp_secs: Some(timestamp_secs),
            nonce: Some(nonce),
            sig_algo_id: Some(sig_algo.wire_id()),
            signature: Some(Cow::Owned(signature)),
        }
    }

    /// Build the canonical byte payload that the server signs and the OBU verifies.
    ///
    /// `[SIGNED_PAYLOAD_TYPE_BYTE][TARGET_OBU_MAC 6B][TIMESTAMP_SECS 8B][NONCE 8B]`
    pub fn build_signed_payload(
        target: MacAddress,
        timestamp_secs: u64,
        nonce: [u8; 8],
    ) -> Vec<u8> {
        let mut payload = vec![SIGNED_PAYLOAD_TYPE_BYTE];
        payload.extend_from_slice(&target.bytes());
        payload.extend_from_slice(&timestamp_secs.to_be_bytes());
        payload.extend_from_slice(&nonce);
        payload
    }

    pub fn target(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.target.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }

    pub fn timestamp_secs(&self) -> Option<u64> {
        self.timestamp_secs
    }

    pub fn nonce(&self) -> Option<&[u8; 8]> {
        self.nonce.as_ref()
    }

    /// Returns the signing algorithm, or `None` if unsigned or unrecognised wire ID.
    pub fn signing_algorithm(&self) -> Option<SigningAlgorithm> {
        self.sig_algo_id.and_then(SigningAlgorithm::from_wire_id)
    }

    pub fn signature(&self) -> Option<&[u8]> {
        self.signature.as_deref()
    }

    /// Wire size in bytes without allocating.
    pub fn wire_size(&self) -> usize {
        // target(6) + optional signed extension: timestamp(8)+nonce(8)+algo(1)+sig_len(2)+sig
        6 + match &self.signature {
            Some(sig) => 8 + 8 + 1 + 2 + sig.len(),
            None => 0,
        }
    }

    /// Clone into an owned version (for forwarding through intermediate OBUs).
    pub fn clone_into_owned(&self) -> SessionTerminated<'static> {
        SessionTerminated {
            target: Cow::Owned(self.target.to_vec()),
            timestamp_secs: self.timestamp_secs,
            nonce: self.nonce,
            sig_algo_id: self.sig_algo_id,
            signature: self.signature.as_ref().map(|s| Cow::Owned(s.to_vec())),
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for SessionTerminated<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.len() < 6 {
            return Err(crate::error::NodeError::BufferTooShort {
                expected: 6,
                actual: value.len(),
            });
        }
        let target = Cow::Borrowed(&value[0..6]);

        // Optional signed extension:
        // [TIMESTAMP 8B][NONCE 8B][SIG_ALGO_ID 1B][SIG_LEN 2B BE][SIG var]
        // Minimum signed total: 6 + 8 + 8 + 1 + 2 = 25 bytes
        if value.len() > 6 {
            if value.len() < 25 {
                return Err(crate::error::NodeError::BufferTooShort {
                    expected: 25,
                    actual: value.len(),
                });
            }
            let timestamp_secs = u64::from_be_bytes(value[6..14].try_into().unwrap());
            let nonce: [u8; 8] = value[14..22].try_into().unwrap();
            let sig_algo_id = value[22];
            let sig_len = u16::from_be_bytes([value[23], value[24]]) as usize;
            if value.len() < 25 + sig_len {
                return Err(crate::error::NodeError::BufferTooShort {
                    expected: 25 + sig_len,
                    actual: value.len(),
                });
            }
            let signature = Cow::Borrowed(&value[25..25 + sig_len]);
            return Ok(Self {
                target,
                timestamp_secs: Some(timestamp_secs),
                nonce: Some(nonce),
                sig_algo_id: Some(sig_algo_id),
                signature: Some(signature),
            });
        }

        Ok(Self {
            target,
            timestamp_secs: None,
            nonce: None,
            sig_algo_id: None,
            signature: None,
        })
    }
}

impl<'a> From<&SessionTerminated<'a>> for Vec<u8> {
    fn from(value: &SessionTerminated<'a>) -> Self {
        let mut out = value.target.to_vec();
        if let (Some(ts), Some(nonce), Some(algo_id), Some(sig)) = (
            value.timestamp_secs,
            value.nonce,
            value.sig_algo_id,
            value.signature.as_deref(),
        ) {
            out.extend_from_slice(&ts.to_be_bytes());
            out.extend_from_slice(&nonce);
            out.push(algo_id);
            let sig_len = sig.len() as u16;
            out.extend_from_slice(&sig_len.to_be_bytes());
            out.extend_from_slice(sig);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsigned_roundtrip() {
        let mac: MacAddress = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66].into();
        let msg = SessionTerminated::new(mac);
        let bytes: Vec<u8> = (&msg).into();
        assert_eq!(bytes.len(), 6);
        let parsed = SessionTerminated::try_from(bytes.as_slice()).unwrap();
        assert_eq!(parsed.target(), mac);
        assert!(parsed.nonce().is_none());
        assert!(parsed.signature().is_none());
    }

    #[test]
    fn signed_roundtrip() {
        let mac: MacAddress = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66].into();
        let ts: u64 = 1_700_000_000;
        let nonce = [0xAAu8; 8];
        let sig = vec![0xBBu8; 64];
        let msg =
            SessionTerminated::new_signed(mac, ts, nonce, SigningAlgorithm::Ed25519, sig.clone());
        let bytes: Vec<u8> = (&msg).into();
        // 6 (mac) + 8 (ts) + 8 (nonce) + 1 (algo) + 2 (len) + 64 (sig) = 89
        assert_eq!(bytes.len(), 89);
        let parsed = SessionTerminated::try_from(bytes.as_slice()).unwrap();
        assert_eq!(parsed.target(), mac);
        assert_eq!(parsed.timestamp_secs(), Some(ts));
        assert_eq!(parsed.nonce(), Some(&nonce));
        assert_eq!(parsed.signing_algorithm(), Some(SigningAlgorithm::Ed25519));
        assert_eq!(parsed.signature(), Some(sig.as_slice()));
    }

    #[test]
    fn too_short_returns_error() {
        assert!(SessionTerminated::try_from(&[0u8; 5][..]).is_err());
    }

    #[test]
    fn signed_too_short_returns_error() {
        // 6 mac bytes + partial signed extension (< 25 total)
        assert!(SessionTerminated::try_from(&[0u8; 10][..]).is_err());
    }

    #[test]
    fn build_signed_payload_format() {
        let mac: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let ts: u64 = 1_700_000_000;
        let nonce = [0xAAu8; 8];
        let payload = SessionTerminated::build_signed_payload(mac, ts, nonce);
        assert_eq!(payload.len(), 1 + 6 + 8 + 8);
        assert_eq!(payload[0], SIGNED_PAYLOAD_TYPE_BYTE);
        assert_eq!(&payload[1..7], &mac.bytes());
        assert_eq!(&payload[7..15], &ts.to_be_bytes());
        assert_eq!(&payload[15..23], &nonce);
    }

    #[test]
    fn clone_into_owned_unsigned() {
        let mac: MacAddress = [1u8; 6].into();
        let original = SessionTerminated::new(mac);
        let bytes: Vec<u8> = (&original).into();
        let borrowed = SessionTerminated::try_from(bytes.as_slice()).unwrap();
        let owned = borrowed.clone_into_owned();
        assert_eq!(owned.target(), mac);
        assert!(owned.nonce().is_none());
        assert!(owned.timestamp_secs().is_none());
    }

    #[test]
    fn clone_into_owned_signed() {
        let mac: MacAddress = [2u8; 6].into();
        let ts: u64 = 1_700_000_001;
        let nonce = [0x55u8; 8];
        let sig = vec![0xCCu8; 64];
        let original =
            SessionTerminated::new_signed(mac, ts, nonce, SigningAlgorithm::Ed25519, sig.clone());
        let bytes: Vec<u8> = (&original).into();
        let borrowed = SessionTerminated::try_from(bytes.as_slice()).unwrap();
        let owned = borrowed.clone_into_owned();
        assert_eq!(owned.target(), mac);
        assert_eq!(owned.timestamp_secs(), Some(ts));
        assert_eq!(owned.nonce(), Some(&nonce));
        assert_eq!(owned.signature(), Some(sig.as_slice()));
    }
}
