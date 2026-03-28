use crate::registry::{self, RegistrationMessage};
use mac_address::MacAddress;

/// Message type byte for upstream data forwarding (RSU -> Server).
pub const UPSTREAM_TYPE: u8 = 0x02;
/// Message type byte for downstream data forwarding (Server -> RSU).
pub const DOWNSTREAM_TYPE: u8 = 0x03;
/// Message type byte for key exchange forwarding (RSU -> Server).
pub const KEY_EXCHANGE_FWD_TYPE: u8 = 0x04;
/// Message type byte for key exchange response (Server -> RSU).
pub const KEY_EXCHANGE_RSP_TYPE: u8 = 0x05;

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
/// Layout: MAGIC(2) + TYPE(1) + OBU_MAC(6) + RSU_MAC(6) + KE_PAYLOAD(42) = 57
pub const KEY_EXCHANGE_FWD_MIN_LEN: usize = 57;

/// Minimum byte length of a KeyExchangeResponse message.
/// Layout: MAGIC(2) + TYPE(1) + OBU_DEST_MAC(6) + KE_PAYLOAD(42) = 51
pub const KEY_EXCHANGE_RSP_MIN_LEN: usize = 51;

/// A key exchange forwarding message sent by an RSU to the server.
///
/// When an RSU receives a `KeyExchangeInit` control message from an OBU on the
/// VANET, it wraps the raw 42-byte key exchange payload in this message and
/// forwards it to the server over UDP.
///
/// Binary format:
/// ```text
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x04] [OBU_MAC 6B] [RSU_MAC 6B] [KE_PAYLOAD 42B]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyExchangeForward {
    /// MAC address of the originating OBU (VANET MAC).
    pub obu_mac: MacAddress,
    /// MAC address of the relaying RSU (VANET MAC).
    pub rsu_mac: MacAddress,
    /// Raw key exchange init payload (42 bytes: key_id + public_key + sender).
    pub payload: Vec<u8>,
}

/// Expected key exchange payload size (key_id 4B + public_key 32B + sender 6B).
pub const KE_PAYLOAD_LEN: usize = 42;

impl KeyExchangeForward {
    pub fn new(obu_mac: MacAddress, rsu_mac: MacAddress, payload: Vec<u8>) -> Self {
        debug_assert_eq!(
            payload.len(),
            KE_PAYLOAD_LEN,
            "KeyExchangeForward payload must be exactly {} bytes",
            KE_PAYLOAD_LEN
        );
        Self {
            obu_mac,
            rsu_mac,
            payload,
        }
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
        if payload.len() != KE_PAYLOAD_LEN {
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
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x05] [OBU_DEST_MAC 6B] [KE_PAYLOAD 42B]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyExchangeResponse {
    /// MAC address of the destination OBU (VANET MAC).
    pub obu_dest_mac: MacAddress,
    /// Raw key exchange reply payload (42 bytes: key_id + public_key + sender).
    pub payload: Vec<u8>,
}

impl KeyExchangeResponse {
    pub fn new(obu_dest_mac: MacAddress, payload: Vec<u8>) -> Self {
        debug_assert_eq!(
            payload.len(),
            KE_PAYLOAD_LEN,
            "KeyExchangeResponse payload must be exactly {} bytes",
            KE_PAYLOAD_LEN
        );
        Self {
            obu_dest_mac,
            payload,
        }
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
        if payload.len() != KE_PAYLOAD_LEN {
            return None;
        }
        let dest_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        Some(Self {
            obu_dest_mac: MacAddress::new(dest_bytes),
            payload: payload.to_vec(),
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
}

impl CloudMessage {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            CloudMessage::Registration(msg) => msg.to_bytes(),
            CloudMessage::UpstreamForward(msg) => msg.to_bytes(),
            CloudMessage::DownstreamForward(msg) => msg.to_bytes(),
            CloudMessage::KeyExchangeForward(msg) => msg.to_bytes(),
            CloudMessage::KeyExchangeResponse(msg) => msg.to_bytes(),
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
        let msg = KeyExchangeForward::new(obu, rsu, payload.clone());
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), KEY_EXCHANGE_FWD_MIN_LEN);
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
        let msg = KeyExchangeResponse::new(dest, payload.clone());
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), KEY_EXCHANGE_RSP_MIN_LEN);
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
        let fwd = KeyExchangeForward::new(obu, rsu, vec![0; 42]);
        let bytes = fwd.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::KeyExchangeForward(fwd));
    }

    #[test]
    fn cloud_message_dispatches_key_exchange_response() {
        let dest: MacAddress = [3u8; 6].into();
        let rsp = KeyExchangeResponse::new(dest, vec![0; 42]);
        let bytes = rsp.to_bytes();
        let parsed = CloudMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, CloudMessage::KeyExchangeResponse(rsp));
    }
}
