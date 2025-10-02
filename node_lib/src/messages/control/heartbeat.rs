use mac_address::MacAddress;
use std::{borrow::Cow, time::Duration};

#[derive(Debug, Clone)]
pub struct Heartbeat<'a> {
    duration: Cow<'a, [u8]>,
    id: Cow<'a, [u8]>,
    hops: Cow<'a, [u8]>,
    source: Cow<'a, [u8]>,
}

impl<'a> Heartbeat<'a> {
    pub fn new(duration: Duration, id: u32, source: MacAddress) -> Self {
        Self {
            duration: Cow::Owned(duration.as_millis().to_be_bytes().to_vec()),
            id: Cow::Owned(id.to_be_bytes().to_vec()),
            hops: Cow::Owned(0u32.to_be_bytes().to_vec()),
            source: Cow::Owned(source.bytes().to_vec()),
        }
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(
            u64::try_from(u128::from_be_bytes(
                unsafe { self.duration.get_unchecked(0..16) }
                    .try_into()
                    .unwrap(),
            ))
            .unwrap(),
        )
    }

    pub fn id(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.id.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn hops(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.hops.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn source(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.source.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }

    /// Internal accessor for zero-copy serialization - returns raw duration bytes
    #[inline]
    pub(crate) fn duration_bytes(&self) -> &[u8] {
        &self.duration
    }

    /// Internal accessor for zero-copy serialization - returns raw id bytes
    #[inline]
    pub(crate) fn id_bytes(&self) -> &[u8] {
        &self.id
    }

    /// Internal accessor for zero-copy serialization - returns raw hops bytes
    #[inline]
    pub(crate) fn hops_bytes(&self) -> &[u8] {
        &self.hops
    }

    /// Internal accessor for zero-copy serialization - returns raw source bytes
    #[inline]
    pub(crate) fn source_bytes(&self) -> &[u8] {
        &self.source
    }
}

impl<'a> TryFrom<&'a [u8]> for Heartbeat<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let source = value
            .get(24..30)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 30,
                actual: value.len(),
            })?;
        let hops = value
            .get(20..24)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 24,
                actual: value.len(),
            })?;
        let id = value
            .get(16..20)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 20,
                actual: value.len(),
            })?;
        let duration = value
            .get(..16)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 16,
                actual: value.len(),
            })?;

        Ok(Self {
            duration: Cow::Borrowed(duration),
            id: Cow::Borrowed(id),
            hops: Cow::Borrowed(hops),
            source: Cow::Borrowed(source),
        })
    }
}

impl<'a> From<&Heartbeat<'a>> for Vec<u8> {
    fn from(value: &Heartbeat<'a>) -> Self {
        let Some(hops) = value.hops.get(0..4) else {
            panic!("did not have hops")
        };
        let hops: [u8; 4] = hops.try_into().expect("convert");
        let hops = u32::from_be_bytes(hops) + 1;

        // Pre-allocate: duration(16) + id(4) + hops(4) + source(6) = 30 bytes
        let mut buf = Vec::with_capacity(30);
        buf.extend_from_slice(&value.duration);
        buf.extend_from_slice(&value.id);
        buf.extend_from_slice(&hops.to_be_bytes());
        buf.extend_from_slice(&value.source);
        buf
    }
}

// Keep backwards compatibility
impl<'a> From<&Heartbeat<'a>> for Vec<Vec<u8>> {
    fn from(value: &Heartbeat<'a>) -> Self {
        let Some(hops) = value.hops.get(0..4) else {
            panic!("did not have hops")
        };
        let hops: [u8; 4] = hops.try_into().expect("convert");
        let hops = u32::from_be_bytes(hops) + 1;
        vec![
            value.duration.to_vec(),
            value.id.to_vec(),
            hops.to_be_bytes().to_vec(),
            value.source.to_vec(),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct HeartbeatReply<'a> {
    duration: Cow<'a, [u8]>,
    id: Cow<'a, [u8]>,
    hops: Cow<'a, [u8]>,
    source: Cow<'a, [u8]>,
    sender: Cow<'a, [u8]>,
}

impl<'a> HeartbeatReply<'a> {
    pub fn from_sender(value: &'a Heartbeat, sender: MacAddress) -> Self {
        Self {
            duration: value.duration.clone(),
            id: value.id.clone(),
            hops: value.hops.clone(),
            source: value.source.clone(),
            sender: Cow::Owned(sender.bytes().to_vec()),
        }
    }

    /// Zero-copy in-place serialization of a HeartbeatReply directly from a borrowed Heartbeat.
    /// This avoids cloning Cow data and allocating intermediate HeartbeatReply.
    ///
    /// # Arguments
    /// * `heartbeat` - The borrowed heartbeat to reply to
    /// * `sender` - The MAC address of the node sending the reply
    /// * `buf` - Pre-allocated buffer to write into (must be at least 36 bytes)
    ///
    /// # Returns
    /// The number of bytes written
    ///
    /// # Performance
    /// This eliminates 2-3 allocations per reply compared to:
    /// ```ignore
    /// let reply = HeartbeatReply::from_sender(hb, mac);
    /// let wire: Vec<u8> = (&reply).into();
    /// ```
    pub fn serialize_from_heartbeat_into(
        heartbeat: &'a Heartbeat,
        sender: MacAddress,
        buf: &mut Vec<u8>,
    ) -> usize {
        buf.clear();
        buf.reserve(36);

        // duration(16) + id(4) + hops(4) + source(6) + sender(6) = 36 bytes
        buf.extend_from_slice(heartbeat.duration_bytes());
        buf.extend_from_slice(heartbeat.id_bytes());
        buf.extend_from_slice(heartbeat.hops_bytes());
        buf.extend_from_slice(heartbeat.source_bytes());
        buf.extend_from_slice(&sender.bytes());

        36
    }

    pub fn duration(&self) -> Duration {
        Duration::from_millis(
            u64::try_from(u128::from_be_bytes(
                unsafe { self.duration.get_unchecked(0..16) }
                    .try_into()
                    .unwrap(),
            ))
            .unwrap(),
        )
    }

    pub fn id(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.id.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn hops(&self) -> u32 {
        u32::from_be_bytes(unsafe { self.hops.get_unchecked(0..4) }.try_into().unwrap())
    }

    pub fn source(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.source.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }

    pub fn sender(&self) -> MacAddress {
        MacAddress::new(
            unsafe { self.sender.get_unchecked(0..6) }
                .try_into()
                .unwrap(),
        )
    }
}

impl<'a> TryFrom<&'a [u8]> for HeartbeatReply<'a> {
    type Error = crate::error::NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        let sender = value
            .get(30..36)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 36,
                actual: value.len(),
            })?;
        let source = value
            .get(24..30)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 30,
                actual: value.len(),
            })?;
        let hops = value
            .get(20..24)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 24,
                actual: value.len(),
            })?;
        let id = value
            .get(16..20)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 20,
                actual: value.len(),
            })?;
        let duration = value
            .get(..16)
            .ok_or_else(|| crate::error::NodeError::BufferTooShort {
                expected: 16,
                actual: value.len(),
            })?;

        Ok(Self {
            duration: Cow::Borrowed(duration),
            id: Cow::Borrowed(id),
            hops: Cow::Borrowed(hops),
            source: Cow::Borrowed(source),
            sender: Cow::Borrowed(sender),
        })
    }
}

impl<'a> From<&HeartbeatReply<'a>> for Vec<u8> {
    fn from(value: &HeartbeatReply<'a>) -> Self {
        // Pre-allocate: duration(16) + id(4) + hops(4) + source(6) + sender(6) = 36 bytes
        let mut buf = Vec::with_capacity(36);
        buf.extend_from_slice(&value.duration);
        buf.extend_from_slice(&value.id);
        buf.extend_from_slice(&value.hops);
        buf.extend_from_slice(&value.source);
        buf.extend_from_slice(&value.sender);
        buf
    }
}

// Keep backwards compatibility
impl<'a> From<&HeartbeatReply<'a>> for Vec<Vec<u8>> {
    fn from(value: &HeartbeatReply<'a>) -> Self {
        vec![
            value.duration.to_vec(),
            value.id.to_vec(),
            value.hops.to_vec(),
            value.source.to_vec(),
            value.sender.to_vec(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::Heartbeat;
    use crate::messages::{control::Control, message::Message, packet_type::PacketType};
    use mac_address::MacAddress;
    use std::time::Duration;

    #[test]
    fn heartbeat_can_be_parsed_and_has_correct_members() {
        // Input packet with hops = [1, 1, 1, 1]
        let pkt_in = vec![
            vec![1u8; 6],
            vec![2u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![0],
            vec![4; 16],
            vec![0; 4],
            vec![1; 4], // Hops = 0x01010101
            vec![2; 6],
        ];
        let apkt: Vec<u8> = pkt_in.iter().flat_map(|x| x.iter()).cloned().collect();
        let msg = Message::try_from(&apkt[..]).expect("is message");
        assert_eq!(msg.from().expect("has from"), MacAddress::new([2u8; 6]));
        assert_eq!(msg.to().expect("has to"), MacAddress::new([1u8; 6]));

        // After serialization, hops should be incremented by 1: [1, 1, 1, 1] -> [1, 1, 1, 2]
        let rpkt: Vec<u8> = (&msg).into();
        let pkt_out = vec![
            vec![1u8; 6],
            vec![2u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![0],
            vec![4; 16],
            vec![0; 4],
            vec![1, 1, 1, 2], // Hops = 0x01010102 (incremented)
            vec![2; 6],
        ];
        let expected: Vec<u8> = pkt_out.iter().flat_map(|x| x.iter()).cloned().collect();
        assert_eq!(expected, rpkt);
    }

    #[test]
    fn create_hearbeat() {
        let msg = Message::new(
            [0; 6].into(),
            [255; 6].into(),
            PacketType::Control(Control::Heartbeat(Heartbeat::new(
                Duration::default(),
                0,
                [4; 6].into(),
            ))),
        );

        let pkt = vec![
            vec![255u8; 6],
            vec![0u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![0],
            vec![0; 16],
            vec![0; 4],
            vec![0, 0, 0, 1],
            vec![4; 6],
        ];
        let apkt: Vec<u8> = pkt.iter().flat_map(|x| x.iter()).cloned().collect();

        let to_vec: Vec<u8> = (&msg).into();
        assert_eq!(to_vec, apkt);
    }

    #[test]
    fn heartbeat_reply_can_be_parsed_and_has_correct_members() {
        let pkt = vec![
            vec![1u8; 6],
            vec![2u8; 6],
            vec![0x30, 0x30],
            vec![0],
            vec![1],
            vec![4; 16],
            vec![0; 4],
            vec![1; 4],
            vec![2; 6],
            vec![3; 6],
        ];
        let apkt: Vec<u8> = pkt.iter().flat_map(|x| x.iter()).cloned().collect();
        let msg = Message::try_from(&apkt[..]).expect("is message");
        assert_eq!(msg.from().expect("has from"), MacAddress::new([2u8; 6]));
        assert_eq!(msg.to().expect("has to"), MacAddress::new([1u8; 6]));

        let rpkt: Vec<u8> = (&msg).into();
        assert_eq!(apkt, rpkt);
    }

    #[test]
    fn heartbeat_try_from_too_short_fails() {
        let pkt = [0u8; 10];
        let res = Heartbeat::try_from(&pkt[..]);
        assert!(res.is_err());
    }

    #[test]
    fn heartbeat_reply_try_from_too_short_fails() {
        let pkt = [0u8; 20];
        let res = crate::messages::control::heartbeat::HeartbeatReply::try_from(&pkt[..]);
        assert!(res.is_err());
    }

    #[test]
    fn zero_copy_heartbeat_reply_serialization_matches_traditional() {
        use super::HeartbeatReply;

        // Create a heartbeat and parse it to get borrowed references
        let hb = Heartbeat::new(Duration::from_millis(100), 42, [1u8; 6].into());
        let hb_msg = Message::new(
            [2u8; 6].into(),
            [255u8; 6].into(),
            PacketType::Control(Control::Heartbeat(hb)),
        );
        let hb_wire: Vec<u8> = (&hb_msg).into();
        let parsed = Message::try_from(&hb_wire[..]).expect("parse");
        let hb_borrowed = match parsed.get_packet_type() {
            PacketType::Control(Control::Heartbeat(h)) => h,
            _ => panic!("wrong type"),
        };

        let sender: MacAddress = [9u8; 6].into();
        let from: MacAddress = [10u8; 6].into();
        let to: MacAddress = [11u8; 6].into();

        // Traditional approach
        let reply_traditional = HeartbeatReply::from_sender(hb_borrowed, sender);
        let msg_traditional = Message::new(
            from,
            to,
            PacketType::Control(Control::HeartbeatReply(reply_traditional)),
        );
        let wire_traditional: Vec<u8> = (&msg_traditional).into();

        // Zero-copy approach
        let mut wire_zero_copy = Vec::new();
        Message::serialize_heartbeat_reply_into(hb_borrowed, sender, from, to, &mut wire_zero_copy);

        // They should produce identical output
        assert_eq!(
            wire_traditional, wire_zero_copy,
            "Zero-copy serialization must produce identical output to traditional approach"
        );

        // Verify it can be parsed back
        let parsed_back = Message::try_from(&wire_zero_copy[..]).expect("parse back");
        assert_eq!(parsed_back.from().unwrap(), from);
        assert_eq!(parsed_back.to().unwrap(), to);

        match parsed_back.get_packet_type() {
            PacketType::Control(Control::HeartbeatReply(hbr)) => {
                assert_eq!(hbr.sender(), sender);
                assert_eq!(hbr.source(), [1u8; 6].into());
                assert_eq!(hbr.id(), 42);
                assert_eq!(hbr.duration(), Duration::from_millis(100));
            }
            _ => panic!("Expected HeartbeatReply"),
        }
    }
}
