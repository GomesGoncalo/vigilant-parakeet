use mac_address::MacAddress;

/// Magic bytes identifying a cloud protocol registration message.
pub const MAGIC: [u8; 2] = [0xAB, 0xCD];
/// Message type byte for RSU registration.
pub const REG_TYPE: u8 = 0x01;
/// Minimum byte length of a valid registration message (no OBUs).
/// Layout: MAGIC(2) + TYPE(1) + RSU_MAC(6) + OBU_COUNT(2) = 11
pub const MIN_LEN: usize = 11;

/// A registration message sent by an RSU to the server over UDP.
///
/// RSUs periodically send this message to inform the server about their
/// identity and the set of OBUs currently associated with them.
///
/// Binary format:
/// ```text
/// [MAGIC 2B: 0xAB 0xCD] [TYPE 1B: 0x01] [RSU_MAC 6B]
/// [OBU_COUNT 2B big-endian] [OBU_MAC_0 6B] ... [OBU_MAC_N-1 6B]
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationMessage {
    /// MAC address of the sending RSU (its VANET interface MAC).
    pub rsu_mac: MacAddress,
    /// MACs of all OBUs currently associated with this RSU.
    pub obu_macs: Vec<MacAddress>,
}

impl RegistrationMessage {
    /// Create a new registration message.
    pub fn new(rsu_mac: MacAddress, obu_macs: Vec<MacAddress>) -> Self {
        Self { rsu_mac, obu_macs }
    }

    /// Serialize to bytes for UDP transmission.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(MIN_LEN + self.obu_macs.len() * 6);
        buf.extend_from_slice(&MAGIC);
        buf.push(REG_TYPE);
        buf.extend_from_slice(&self.rsu_mac.bytes());
        let count = self.obu_macs.len() as u16;
        buf.extend_from_slice(&count.to_be_bytes());
        for mac in &self.obu_macs {
            buf.extend_from_slice(&mac.bytes());
        }
        buf
    }

    /// Parse a registration message from raw bytes. Returns `None` on invalid input.
    pub fn try_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < MIN_LEN {
            return None;
        }
        if data[0..2] != MAGIC {
            return None;
        }
        if data[2] != REG_TYPE {
            return None;
        }
        let rsu_bytes: [u8; 6] = data[3..9].try_into().ok()?;
        let rsu_mac = MacAddress::new(rsu_bytes);
        let count = u16::from_be_bytes([data[9], data[10]]) as usize;
        let expected_len = MIN_LEN + count * 6;
        if data.len() < expected_len {
            return None;
        }
        let mut obu_macs = Vec::with_capacity(count);
        for i in 0..count {
            let start = MIN_LEN + i * 6;
            let obu_bytes: [u8; 6] = data[start..start + 6].try_into().ok()?;
            obu_macs.push(MacAddress::new(obu_bytes));
        }
        Some(Self { rsu_mac, obu_macs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mac_address::MacAddress;

    #[test]
    fn roundtrip_no_obus() {
        let rsu: MacAddress = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF].into();
        let msg = RegistrationMessage::new(rsu, vec![]);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), MIN_LEN);
        let parsed = RegistrationMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn roundtrip_with_obus() {
        let rsu: MacAddress = [1u8; 6].into();
        let obus = vec![MacAddress::new([2u8; 6]), MacAddress::new([3u8; 6])];
        let msg = RegistrationMessage::new(rsu, obus);
        let bytes = msg.to_bytes();
        assert_eq!(bytes.len(), MIN_LEN + 2 * 6);
        let parsed = RegistrationMessage::try_from_bytes(&bytes).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn too_short_returns_none() {
        assert!(RegistrationMessage::try_from_bytes(&[0xAB, 0xCD, 0x01]).is_none());
    }

    #[test]
    fn wrong_magic_returns_none() {
        let rsu: MacAddress = [1u8; 6].into();
        let mut bytes = RegistrationMessage::new(rsu, vec![]).to_bytes();
        bytes[0] = 0x00;
        assert!(RegistrationMessage::try_from_bytes(&bytes).is_none());
    }

    #[test]
    fn wrong_type_returns_none() {
        let rsu: MacAddress = [1u8; 6].into();
        let mut bytes = RegistrationMessage::new(rsu, vec![]).to_bytes();
        bytes[2] = 0xFF;
        assert!(RegistrationMessage::try_from_bytes(&bytes).is_none());
    }

    #[test]
    fn truncated_obu_list_returns_none() {
        let rsu: MacAddress = [1u8; 6].into();
        let obus = vec![MacAddress::new([2u8; 6])];
        let mut bytes = RegistrationMessage::new(rsu, obus).to_bytes();
        // claim 2 OBUs but only 1 is present
        bytes[9] = 0x00;
        bytes[10] = 0x02;
        assert!(RegistrationMessage::try_from_bytes(&bytes).is_none());
    }
}
