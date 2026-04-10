use crate::registry::{self, RegistrationMessage};
use anyhow::{bail, Result};
use mac_address::MacAddress;
use node_lib::crypto::SigningAlgorithm;

/// Message type byte for upstream data forwarding (RSU -> Server).
pub const UPSTREAM_TYPE: u8 = 0x02;
/// Message type byte for downstream data forwarding (Server -> RSU).
pub const DOWNSTREAM_TYPE: u8 = 0x03;
/// Message type byte for key exchange forwarding (RSU -> Server).
pub const KEY_EXCHANGE_FWD_TYPE: u8 = 0x04;
/// Message type byte for key exchange response (Server -> RSU).
pub const KEY_EXCHANGE_RSP_TYPE: u8 = 0x05;
/// Message type byte for session termination notification (Server -> RSU -> OBU).
pub const SESSION_TERMINATED_TYPE: u8 = 0x06;

/// Minimum byte length of an UpstreamForward message (no payload).
/// Layout: MAGIC(2) + TYPE(1) + RSU_MAC(6) + OBU_SOURCE_MAC(6) = 15
pub const UPSTREAM_MIN_LEN: usize = 15;

/// Minimum byte length of a DownstreamForward message (no payload).
/// Layout: MAGIC(2) + TYPE(1) + OBU_DEST_MAC(6) + ORIGIN_MAC(6) = 15
pub const DOWNSTREAM_MIN_LEN: usize = 15;

/// An upstream data forwarding message sent by an RSU to the server.
///
/// When an RSU receives upstream data from an OBU on the VANET, it wraps
/// the raw (still-encrypted) payload in this message and forwards it to
/// the server over UDP.
///
/// Binary format:
/// ```text
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x02] [RSU_MAC 6B] [OBU_SOURCE_MAC 6B] [PAYLOAD ...]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamForward {
    /// MAC address of the forwarding RSU (its VANET interface MAC).
    pub rsu_mac: MacAddress,
    /// MAC address of the originating OBU.
    pub obu_source_mac: MacAddress,
    /// Raw payload (encrypted by OBU, passed through opaquely by RSU).
    pub payload: Vec<u8>,
}

impl UpstreamForward {
    pub fn new(rsu_mac: MacAddress, obu_source_mac: MacAddress, payload: Vec<u8>) -> Self {
        Self {
            rsu_mac,
            obu_source_mac,
            payload,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(UPSTREAM_MIN_LEN + self.payload.len());
        buf.extend_from_slice(&registry::MAGIC);
        buf.push(UPSTREAM_TYPE);
        buf.extend_from_slice(&self.rsu_mac.bytes());
        buf.extend_from_slice(&self.obu_source_mac.bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < UPSTREAM_MIN_LEN {
            return None;
        }
        if data[0..2] != registry::MAGIC {
            return None;
        }
        if data[2] != UPSTREAM_TYPE {
            return None;
        }
        let rsu_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        let rsu_mac = MacAddress::new(rsu_bytes);
        let obu_bytes: [u8; 6] = data[9..15].try_into().ok()?;
        let obu_source_mac = MacAddress::new(obu_bytes);
        let payload = data[15..].to_vec();
        Some(Self {
            rsu_mac,
            obu_source_mac,
            payload,
        })
    }
}

/// A downstream data forwarding message sent by the server to an RSU.
///
/// The server sends this when it has data destined for a specific OBU.
/// The RSU receives it, constructs a VANET Downstream message, and
/// delivers it to the OBU over the wireless medium.
///
/// Binary format:
/// ```text
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x03] [OBU_DEST_MAC 6B] [ORIGIN_MAC 6B] [PAYLOAD ...]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownstreamForward {
    /// MAC address of the destination OBU.
    pub obu_dest_mac: MacAddress,
    /// Origin MAC address (source identifier for the VANET Downstream message).
    pub origin_mac: MacAddress,
    /// Encrypted payload for the destination OBU.
    pub payload: Vec<u8>,
}

impl DownstreamForward {
    pub fn new(obu_dest_mac: MacAddress, origin_mac: MacAddress, payload: Vec<u8>) -> Self {
        Self {
            obu_dest_mac,
            origin_mac,
            payload,
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(DOWNSTREAM_MIN_LEN + self.payload.len());
        buf.extend_from_slice(&registry::MAGIC);
        buf.push(DOWNSTREAM_TYPE);
        buf.extend_from_slice(&self.obu_dest_mac.bytes());
        buf.extend_from_slice(&self.origin_mac.bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < DOWNSTREAM_MIN_LEN {
            return None;
        }
        if data[0..2] != registry::MAGIC {
            return None;
        }
        if data[2] != DOWNSTREAM_TYPE {
            return None;
        }
        let dest_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        let obu_dest_mac = MacAddress::new(dest_bytes);
        let origin_bytes: [u8; 6] = data[9..15].try_into().ok()?;
        let origin_mac = MacAddress::new(origin_bytes);
        let payload = data[15..].to_vec();
        Some(Self {
            obu_dest_mac,
            origin_mac,
            payload,
        })
    }
}

/// Minimum byte length of a KeyExchangeForward message.
/// Layout: MAGIC(2) + TYPE(1) + OBU_MAC(6) + RSU_MAC(6) + KE_PAYLOAD(≥14) = 29
pub const KEY_EXCHANGE_FWD_MIN_LEN: usize = 29;

/// Minimum byte length of a KeyExchangeResponse message.
/// Layout: MAGIC(2) + TYPE(1) + OBU_DEST_MAC(6) + KE_PAYLOAD(≥14) = 23
pub const KEY_EXCHANGE_RSP_MIN_LEN: usize = 23;

/// Minimum key exchange payload size.
/// New variable-length format: algo_id(1) + key_id(4) + km_len(2) + km(≥1) + sender(6) = 14
pub const KE_PAYLOAD_MIN_LEN: usize = 14;

/// A key exchange forwarding message sent by an RSU to the server.
///
/// When an RSU receives a `KeyExchangeInit` control message from an OBU on the
/// VANET, it wraps the variable-length key exchange payload in this message and
/// forwards it to the server over UDP.
///
/// Binary format:
/// ```text
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x04] [OBU_MAC 6B] [RSU_MAC 6B] [KE_PAYLOAD ≥14B]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyExchangeForward {
    /// MAC address of the originating OBU (VANET MAC).
    pub obu_mac: MacAddress,
    /// MAC address of the relaying RSU (VANET MAC).
    pub rsu_mac: MacAddress,
    /// Raw key exchange init payload (variable length, new format).
    pub payload: Vec<u8>,
}

impl KeyExchangeForward {
    pub fn new(obu_mac: MacAddress, rsu_mac: MacAddress, payload: Vec<u8>) -> Result<Self> {
        if payload.len() < KE_PAYLOAD_MIN_LEN {
            bail!(
                "KeyExchangeForward payload must be at least {} bytes, got {}",
                KE_PAYLOAD_MIN_LEN,
                payload.len()
            );
        }
        Ok(Self {
            obu_mac,
            rsu_mac,
            payload,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(15 + self.payload.len());
        buf.extend_from_slice(&registry::MAGIC);
        buf.push(KEY_EXCHANGE_FWD_TYPE);
        buf.extend_from_slice(&self.obu_mac.bytes());
        buf.extend_from_slice(&self.rsu_mac.bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < KEY_EXCHANGE_FWD_MIN_LEN {
            return None;
        }
        if data[0..2] != registry::MAGIC || data[2] != KEY_EXCHANGE_FWD_TYPE {
            return None;
        }
        let payload = &data[15..];
        if payload.len() < KE_PAYLOAD_MIN_LEN {
            return None;
        }
        let obu_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        let rsu_bytes: [u8; 6] = data[9..15].try_into().ok()?;
        Some(Self {
            obu_mac: MacAddress::new(obu_bytes),
            rsu_mac: MacAddress::new(rsu_bytes),
            payload: payload.to_vec(),
        })
    }
}

/// A key exchange response message sent by the server to an RSU.
///
/// The server sends this after handling a `KeyExchangeForward`. The RSU
/// then constructs a VANET `KeyExchangeReply` control message and delivers
/// it to the target OBU.
///
/// Binary format:
/// ```text
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x05] [OBU_DEST_MAC 6B] [KE_PAYLOAD ≥14B]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyExchangeResponse {
    /// MAC address of the destination OBU (VANET MAC).
    pub obu_dest_mac: MacAddress,
    /// Raw key exchange reply payload (variable length, new format).
    pub payload: Vec<u8>,
}

impl KeyExchangeResponse {
    pub fn new(obu_dest_mac: MacAddress, payload: Vec<u8>) -> Result<Self> {
        if payload.len() < KE_PAYLOAD_MIN_LEN {
            bail!(
                "KeyExchangeResponse payload must be at least {} bytes, got {}",
                KE_PAYLOAD_MIN_LEN,
                payload.len()
            );
        }
        Ok(Self {
            obu_dest_mac,
            payload,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(9 + self.payload.len());
        buf.extend_from_slice(&registry::MAGIC);
        buf.push(KEY_EXCHANGE_RSP_TYPE);
        buf.extend_from_slice(&self.obu_dest_mac.bytes());
        buf.extend_from_slice(&self.payload);
        buf
    }

    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < KEY_EXCHANGE_RSP_MIN_LEN {
            return None;
        }
        if data[0..2] != registry::MAGIC || data[2] != KEY_EXCHANGE_RSP_TYPE {
            return None;
        }
        let payload = &data[9..];
        if payload.len() < KE_PAYLOAD_MIN_LEN {
            return None;
        }
        let dest_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        Some(Self {
            obu_dest_mac: MacAddress::new(dest_bytes),
            payload: payload.to_vec(),
        })
    }
}

/// A session termination notification sent by the server to an RSU.
///
/// The RSU relays this as a VANET `SessionTerminated` control message to the
/// target OBU.  The OBU clears its DH session key and immediately re-initiates
/// key exchange.
///
/// Binary format:
/// ```text
/// Unsigned: [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x06] [OBU_MAC 6B]
/// Signed:   [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x06] [OBU_MAC 6B]
///           [TIMESTAMP_SECS 8B] [NONCE 8B] [SIG_ALGO_ID 1B] [SIG_LEN 2B BE] [SIGNATURE var]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTerminatedForward {
    /// VANET MAC of the OBU whose session was revoked.
    pub obu_mac: MacAddress,
    /// Unix timestamp (seconds) when the server issued the revocation.
    pub timestamp_secs: Option<u64>,
    /// 8-byte server-generated random nonce.
    pub nonce: Option<[u8; 8]>,
    /// Signature algorithm (optional; same algorithms as KeyExchange).
    pub sig_algo: Option<SigningAlgorithm>,
    /// Raw signature over `[0x04][OBU_MAC 6B][TIMESTAMP_SECS 8B][NONCE 8B]`.
    pub signature: Option<Vec<u8>>,
}

impl SessionTerminatedForward {
    pub fn new(obu_mac: MacAddress) -> Self {
        Self {
            obu_mac,
            timestamp_secs: None,
            nonce: None,
            sig_algo: None,
            signature: None,
        }
    }

    pub fn new_signed(
        obu_mac: MacAddress,
        timestamp_secs: u64,
        nonce: [u8; 8],
        sig_algo: SigningAlgorithm,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            obu_mac,
            timestamp_secs: Some(timestamp_secs),
            nonce: Some(nonce),
            sig_algo: Some(sig_algo),
            signature: Some(signature),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(9);
        buf.extend_from_slice(&registry::MAGIC);
        buf.push(SESSION_TERMINATED_TYPE);
        buf.extend_from_slice(&self.obu_mac.bytes());
        if let (Some(ts), Some(nonce), Some(algo), Some(sig)) = (
            self.timestamp_secs,
            self.nonce,
            self.sig_algo,
            self.signature.as_deref(),
        ) {
            buf.extend_from_slice(&ts.to_be_bytes());
            buf.extend_from_slice(&nonce);
            buf.push(algo.wire_id());
            let sig_len = sig.len() as u16;
            buf.extend_from_slice(&sig_len.to_be_bytes());
            buf.extend_from_slice(sig);
        }
        buf
    }

    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 9 {
            return None;
        }
        if data[0..2] != registry::MAGIC || data[2] != SESSION_TERMINATED_TYPE {
            return None;
        }
        let obu_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        let obu_mac = MacAddress::new(obu_bytes);

        // Optional signed extension:
        // [TIMESTAMP 8B][NONCE 8B][SIG_ALGO_ID 1B][SIG_LEN 2B BE][SIG var]
        // Minimum total: 9 + 8 + 8 + 1 + 2 = 28 bytes
        if data.len() > 9 {
            if data.len() < 28 {
                return None;
            }
            let timestamp_secs = u64::from_be_bytes(data[9..17].try_into().ok()?);
            let nonce: [u8; 8] = data[17..25].try_into().ok()?;
            let sig_algo = SigningAlgorithm::from_wire_id(data[25])?;
            let sig_len = u16::from_be_bytes([data[26], data[27]]) as usize;
            if data.len() < 28 + sig_len {
                return None;
            }
            let signature = data[28..28 + sig_len].to_vec();
            return Some(Self {
                obu_mac,
                timestamp_secs: Some(timestamp_secs),
                nonce: Some(nonce),
                sig_algo: Some(sig_algo),
                signature: Some(signature),
            });
        }

        Some(Self {
            obu_mac,
            timestamp_secs: None,
            nonce: None,
            sig_algo: None,
            signature: None,
        })
    }
}

/// Unified cloud protocol message enum.
///
/// All messages between RSU and Server share the `[0xAB, 0xCD]` magic prefix
/// and are distinguished by the type byte at offset 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudMessage {
    Registration(RegistrationMessage),
    UpstreamForward(UpstreamForward),
    DownstreamForward(DownstreamForward),
    KeyExchangeForward(KeyExchangeForward),
    KeyExchangeResponse(KeyExchangeResponse),
    SessionTerminatedForward(SessionTerminatedForward),
}

impl CloudMessage {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            CloudMessage::Registration(msg) => msg.to_bytes(),
            CloudMessage::UpstreamForward(msg) => msg.to_bytes(),
            CloudMessage::DownstreamForward(msg) => msg.to_bytes(),
            CloudMessage::KeyExchangeForward(msg) => msg.to_bytes(),
            CloudMessage::KeyExchangeResponse(msg) => msg.to_bytes(),
            CloudMessage::SessionTerminatedForward(msg) => msg.to_bytes(),
        }
    }

    /// Parse a cloud protocol message from raw bytes.
    /// Returns `None` if the data is too short, has wrong magic, or unknown type.
    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 3 {
            return None;
        }
        if data[0..2] != registry::MAGIC {
            return None;
        }
        match data[2] {
            registry::REG_TYPE => {
                RegistrationMessage::try_from_bytes(data).map(CloudMessage::Registration)
            }
            UPSTREAM_TYPE => {
                UpstreamForward::try_from_bytes(data).map(CloudMessage::UpstreamForward)
            }
            DOWNSTREAM_TYPE => {
                DownstreamForward::try_from_bytes(data).map(CloudMessage::DownstreamForward)
            }
            KEY_EXCHANGE_FWD_TYPE => {
                KeyExchangeForward::try_from_bytes(data).map(CloudMessage::KeyExchangeForward)
            }
            KEY_EXCHANGE_RSP_TYPE => {
                KeyExchangeResponse::try_from_bytes(data).map(CloudMessage::KeyExchangeResponse)
            }
            SESSION_TERMINATED_TYPE => SessionTerminatedForward::try_from_bytes(data)
                .map(CloudMessage::SessionTerminatedForward),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_forward_roundtrip() {
        let rsu: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let obu: MacAddress = [1u8; 6].into();
        let payload = vec![10, 20, 30, 40, 50];
        let msg = UpstreamForward::new(rsu, obu, payload.clone());
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), UPSTREAM_MIN_LEN + payload.len());
        let parsed = UpstreamForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn upstream_forward_empty_payload() {
        let rsu: MacAddress = [2u8; 6].into();
        let obu: MacAddress = [3u8; 6].into();
        let msg = UpstreamForward::new(rsu, obu, vec![]);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), UPSTREAM_MIN_LEN);
        let parsed = UpstreamForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn upstream_forward_too_short() {
        assert!(UpstreamForward::try_from_bytes(&[0xAB, 0xCD, 0x02]).is_none());
    }

    #[test]
    fn downstream_forward_roundtrip() {
        let dest: MacAddress = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66].into();
        let origin: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let payload = vec![99, 88, 77];
        let msg = DownstreamForward::new(dest, origin, payload.clone());
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), DOWNSTREAM_MIN_LEN + payload.len());
        let parsed = DownstreamForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn downstream_forward_empty_payload() {
        let dest: MacAddress = [4u8; 6].into();
        let origin: MacAddress = [5u8; 6].into();
        let msg = DownstreamForward::new(dest, origin, vec![]);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), DOWNSTREAM_MIN_LEN);
        let parsed = DownstreamForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn downstream_forward_too_short() {
        assert!(DownstreamForward::try_from_bytes(&[0xAB, 0xCD, 0x03]).is_none());
    }

    #[test]
    fn cloud_message_dispatches_registration() {
        let rsu: MacAddress = [1u8; 6].into();
        let reg = RegistrationMessage::new(rsu, vec![]);
        let bytes = reg.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::Registration(reg));
    }

    #[test]
    fn cloud_message_dispatches_upstream() {
        let rsu: MacAddress = [2u8; 6].into();
        let obu: MacAddress = [3u8; 6].into();
        let fwd = UpstreamForward::new(rsu, obu, vec![1, 2, 3]);
        let bytes = fwd.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::UpstreamForward(fwd));
    }

    #[test]
    fn cloud_message_dispatches_downstream() {
        let dest: MacAddress = [4u8; 6].into();
        let origin: MacAddress = [5u8; 6].into();
        let fwd = DownstreamForward::new(dest, origin, vec![7, 8, 9]);
        let bytes = fwd.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::DownstreamForward(fwd));
    }

    #[test]
    fn cloud_message_unknown_type_returns_none() {
        let data = [0xAB, 0xCD, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(CloudMessage::try_from_bytes(&data).is_none());
    }

    #[test]
    fn cloud_message_too_short_returns_none() {
        assert!(CloudMessage::try_from_bytes(&[0xAB]).is_none());
        assert!(CloudMessage::try_from_bytes(&[]).is_none());
    }

    #[test]
    fn cloud_message_wrong_magic_returns_none() {
        let data = [0x00, 0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(CloudMessage::try_from_bytes(&data).is_none());
    }

    #[test]
    fn key_exchange_forward_roundtrip() {
        let obu: MacAddress = [1u8; 6].into();
        let rsu: MacAddress = [2u8; 6].into();
        let payload = vec![0xAB; 42];
        let msg = KeyExchangeForward::new(obu, rsu, payload.clone()).unwrap();
        let bytes = msg.to_bytes();
        // header (15 bytes: MAGIC+TYPE+OBU_MAC+RSU_MAC) + payload
        assert_eq!(bytes.len(), 15 + payload.len());
        let parsed = KeyExchangeForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn key_exchange_forward_signed_roundtrip() {
        let obu: MacAddress = [1u8; 6].into();
        let rsu: MacAddress = [2u8; 6].into();
        let payload = vec![0xAB; 138];
        let msg = KeyExchangeForward::new(obu, rsu, payload.clone()).unwrap();
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 15 + 138);
        let parsed = KeyExchangeForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn key_exchange_forward_too_short() {
        assert!(KeyExchangeForward::try_from_bytes(&[0xAB, 0xCD, 0x04]).is_none());
    }

    #[test]
    fn key_exchange_response_roundtrip() {
        let dest: MacAddress = [3u8; 6].into();
        let payload = vec![0xCD; 42];
        let msg = KeyExchangeResponse::new(dest, payload.clone()).unwrap();
        let bytes = msg.to_bytes();
        // header (9 bytes: MAGIC+TYPE+OBU_DEST_MAC) + payload
        assert_eq!(bytes.len(), 9 + payload.len());
        let parsed = KeyExchangeResponse::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn key_exchange_response_signed_roundtrip() {
        let dest: MacAddress = [3u8; 6].into();
        let payload = vec![0xCD; 138];
        let msg = KeyExchangeResponse::new(dest, payload.clone()).unwrap();
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 9 + 138);
        let parsed = KeyExchangeResponse::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn key_exchange_response_too_short() {
        assert!(KeyExchangeResponse::try_from_bytes(&[0xAB, 0xCD, 0x05]).is_none());
    }

    #[test]
    fn cloud_message_dispatches_key_exchange_forward() {
        let obu: MacAddress = [1u8; 6].into();
        let rsu: MacAddress = [2u8; 6].into();
        let fwd = KeyExchangeForward::new(obu, rsu, vec![0; 42]).unwrap();
        let bytes = fwd.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::KeyExchangeForward(fwd));
    }

    #[test]
    fn cloud_message_dispatches_key_exchange_response() {
        let dest: MacAddress = [3u8; 6].into();
        let rsp = KeyExchangeResponse::new(dest, vec![0; 42]).unwrap();
        let bytes = rsp.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::KeyExchangeResponse(rsp));
    }

    #[test]
    fn session_terminated_forward_roundtrip() {
        let obu: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let msg = SessionTerminatedForward::new(obu);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), 9);
        let parsed = SessionTerminatedForward::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed.obu_mac, obu);
    }

    #[test]
    fn cloud_message_dispatches_session_terminated_forward() {
        let obu: MacAddress = [5u8; 6].into();
        let msg = SessionTerminatedForward::new(obu);
        let bytes = msg.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::SessionTerminatedForward(msg));
    }

    #[test]
    fn session_terminated_forward_too_short() {
        assert!(SessionTerminatedForward::try_from_bytes(&[0xAB, 0xCD, 0x06]).is_none());
    }
}
