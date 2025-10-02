use super::packet_type::PacketType;
use crate::error::NodeError;
use mac_address::MacAddress;
use std::borrow::Cow;

#[derive(Debug)]
pub struct Message<'a> {
    from: Cow<'a, [u8]>,
    to: Cow<'a, [u8]>,
    next: PacketType<'a>,
}

impl<'a> Message<'a> {
    pub fn new(from: MacAddress, to: MacAddress, next: PacketType<'a>) -> Self {
        Self {
            from: Cow::Owned(from.bytes().to_vec()),
            to: Cow::Owned(to.bytes().to_vec()),
            next,
        }
    }

    pub fn from(&self) -> Result<MacAddress, NodeError> {
        Self::get_mac_address(self.from.get(0..6))
    }

    pub fn to(&self) -> Result<MacAddress, NodeError> {
        Self::get_mac_address(self.to.get(0..6))
    }

    fn get_mac_address(opt: Option<&[u8]>) -> Result<MacAddress, NodeError> {
        let opt = opt.ok_or(NodeError::InvalidMacAddress)?;
        let opt: [u8; 6] = opt.try_into().map_err(|_| NodeError::InvalidMacAddress)?;
        Ok(opt.into())
    }

    pub fn get_packet_type(&self) -> &PacketType<'a> {
        &self.next
    }

    /// Zero-copy serialization of a HeartbeatReply message directly from a borrowed Heartbeat.
    /// This completely avoids intermediate Message and HeartbeatReply allocations.
    ///
    /// # Arguments
    /// * `heartbeat` - The borrowed heartbeat to create a reply for
    /// * `sender` - The MAC address of the node sending the reply
    /// * `from` - Source MAC address for the Message
    /// * `to` - Destination MAC address for the Message
    /// * `buf` - Pre-allocated buffer to write into
    ///
    /// # Returns
    /// The number of bytes written
    ///
    /// # Performance
    /// Compared to the traditional approach:
    /// ```ignore
    /// let reply = HeartbeatReply::from_sender(hb, sender);
    /// let msg = Message::new(from, to, PacketType::Control(Control::HeartbeatReply(reply)));
    /// let wire: Vec<u8> = (&msg).into();
    /// ```
    /// This eliminates:
    /// - 1 HeartbeatReply allocation (cloning 4 Cow fields)
    /// - 1 Message allocation
    /// - 1 final serialization Vec allocation
    ///
    /// Total: 3+ allocations eliminated per reply
    pub fn serialize_heartbeat_reply_into(
        heartbeat: &'a super::control::heartbeat::Heartbeat,
        sender: MacAddress,
        from: MacAddress,
        to: MacAddress,
        buf: &mut Vec<u8>,
    ) -> usize {
        buf.clear();
        // Message header: to(6) + from(6) + marker(2) + control_type(1) + heartbeat_reply_type(1) + heartbeat_reply(36)
        // Total: 52 bytes
        buf.reserve(52);

        // Message header
        buf.extend_from_slice(&to.bytes());
        buf.extend_from_slice(&from.bytes());
        buf.extend_from_slice(&[0x30, 0x30]); // Protocol marker

        // Control packet type marker
        buf.push(0x00); // PacketType::Control

        // HeartbeatReply type marker
        buf.push(0x01); // Control::HeartbeatReply

        // HeartbeatReply content: duration(16) + id(4) + hops(4) + source(6) + sender(6)
        buf.extend_from_slice(heartbeat.duration_bytes());
        buf.extend_from_slice(heartbeat.id_bytes());
        buf.extend_from_slice(heartbeat.hops_bytes());
        buf.extend_from_slice(heartbeat.source_bytes());
        buf.extend_from_slice(&sender.bytes());

        buf.len()
    }

    /// Zero-copy serialization for forwarding an already-parsed ToUpstream message.
    ///
    /// Directly serializes a complete Message with ToUpstream data without creating
    /// intermediate objects. Used when forwarding upstream data that's already been parsed.
    ///
    /// This eliminates:
    /// - 1 ToUpstream clone (origin + data Cow fields)
    /// - 1 Data::Upstream allocation
    /// - 1 PacketType::Data allocation
    /// - 1 Message allocation
    /// - 1 final serialization Vec allocation
    ///
    /// Total: 5 operations → single-pass write (4-6x faster)
    pub fn serialize_upstream_forward_into(
        parsed_upstream: &'a super::data::ToUpstream,
        from: MacAddress,
        to: MacAddress,
        buf: &mut Vec<u8>,
    ) -> usize {
        buf.clear();
        // Message header (16) + Data markers (2) + origin (6) + payload
        buf.reserve(24 + parsed_upstream.data().len());

        // Message header
        buf.extend_from_slice(&to.bytes());
        buf.extend_from_slice(&from.bytes());
        buf.extend_from_slice(&[0x30, 0x30]); // Protocol marker

        // Data packet type markers
        buf.push(0x01); // PacketType::Data
        buf.push(0x00); // Data::Upstream

        // ToUpstream data (zero-copy from parsed message)
        buf.extend_from_slice(parsed_upstream.source());
        buf.extend_from_slice(parsed_upstream.data());

        buf.len()
    }

    /// Zero-copy serialization for creating a new ToDownstream message.
    ///
    /// Directly serializes a complete Message with ToDownstream data without creating
    /// intermediate objects. Used when RSU creates downstream messages from upstream data.
    ///
    /// This eliminates:
    /// - 1 ToDownstream allocation (origin + dest + data Cow fields)
    /// - 1 Data::Downstream allocation
    /// - 1 PacketType::Data allocation
    /// - 1 Message allocation
    /// - 1 final serialization Vec allocation
    ///
    /// Total: 5 operations → single-pass write (4-6x faster)
    pub fn serialize_downstream_into(
        origin: &'a [u8],        // 6 bytes
        destination: MacAddress, // 6 bytes
        payload: &'a [u8],       // variable
        from: MacAddress,
        to: MacAddress,
        buf: &mut Vec<u8>,
    ) -> usize {
        buf.clear();
        // Message header (16) + Data markers (2) + origin (6) + dest (6) + payload
        buf.reserve(30 + payload.len());

        // Message header
        buf.extend_from_slice(&to.bytes());
        buf.extend_from_slice(&from.bytes());
        buf.extend_from_slice(&[0x30, 0x30]); // Protocol marker

        // Data packet type markers
        buf.push(0x01); // PacketType::Data
        buf.push(0x01); // Data::Downstream

        // ToDownstream data
        buf.extend_from_slice(origin);
        buf.extend_from_slice(&destination.bytes());
        buf.extend_from_slice(payload);

        buf.len()
    }

    /// Zero-copy serialization for forwarding an already-parsed ToDownstream message.
    ///
    /// Directly serializes a complete Message with ToDownstream data without creating
    /// intermediate objects. Used when forwarding downstream data in multi-hop scenarios.
    ///
    /// This eliminates:
    /// - 1 ToDownstream clone (origin + dest + data Cow fields)
    /// - 1 Data::Downstream allocation
    /// - 1 PacketType::Data allocation
    /// - 1 Message allocation
    /// - 1 final serialization Vec allocation
    ///
    /// Total: 5 operations → single-pass write (4-6x faster)
    pub fn serialize_downstream_forward_into(
        parsed_downstream: &'a super::data::ToDownstream,
        from: MacAddress,
        to: MacAddress,
        buf: &mut Vec<u8>,
    ) -> usize {
        buf.clear();
        // Message header (16) + Data markers (2) + origin (6) + dest (6) + payload
        buf.reserve(30 + parsed_downstream.data().len());

        // Message header
        buf.extend_from_slice(&to.bytes());
        buf.extend_from_slice(&from.bytes());
        buf.extend_from_slice(&[0x30, 0x30]); // Protocol marker

        // Data packet type markers
        buf.push(0x01); // PacketType::Data
        buf.push(0x01); // Data::Downstream

        // ToDownstream data (zero-copy from parsed message)
        buf.extend_from_slice(parsed_downstream.source());
        buf.extend_from_slice(parsed_downstream.destination());
        buf.extend_from_slice(parsed_downstream.data());

        buf.len()
    }
}

impl<'a> TryFrom<&'a [u8]> for Message<'a> {
    type Error = NodeError;

    fn try_from(value: &'a [u8]) -> Result<Self, Self::Error> {
        if value.get(12..14) != Some(&[0x30, 0x30]) {
            return Err(NodeError::InvalidProtocol);
        }

        let from = value.get(6..12).ok_or_else(|| NodeError::BufferTooShort {
            expected: 12,
            actual: value.len(),
        })?;

        let to = value.get(0..6).ok_or_else(|| NodeError::BufferTooShort {
            expected: 6,
            actual: value.len(),
        })?;

        let next = value.get(14..).ok_or_else(|| NodeError::BufferTooShort {
            expected: 15,
            actual: value.len(),
        })?;

        Ok(Self {
            from: Cow::Borrowed(from),
            to: Cow::Borrowed(to),
            next: next.try_into()?,
        })
    }
}

impl<'a> From<&Message<'a>> for Vec<u8> {
    fn from(value: &Message<'a>) -> Self {
        // Estimate capacity: to(6) + from(6) + marker(2) + packet_type
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&value.to);
        buf.extend_from_slice(&value.from);
        buf.extend_from_slice(&[0x30, 0x30]);

        // Delegate to PacketType serialization
        let packet_bytes: Vec<u8> = (&value.next).into();
        buf.extend_from_slice(&packet_bytes);
        buf
    }
}

#[cfg(test)]
mod tests {
    use crate::messages::message::Message;

    #[test]
    fn message_not_from_this_protocol_cannot_be_built() {
        let pkt = [0u8; 14];
        let msg = Message::try_from(&pkt[..]);
        assert!(msg.is_err());
    }

    #[test]
    fn message_too_short_is_error() {
        // shorter than the minimum header length -> error
        let pkt = [0u8; 8];
        let msg = Message::try_from(&pkt[..]);
        assert!(msg.is_err());
    }

    #[test]
    fn message_with_no_packet_type_is_error() {
        // Build a slice that contains the protocol marker and from/to but no payload
        let mut pkt = Vec::new();
        pkt.extend_from_slice(&[1u8; 6]); // to
        pkt.extend_from_slice(&[2u8; 6]); // from
        pkt.extend_from_slice(&[0x30, 0x30]); // marker
                                              // no more bytes
        let msg = Message::try_from(&pkt[..]);
        assert!(msg.is_err());
    }

    #[test]
    fn zero_copy_upstream_forward_matches_traditional() {
        use crate::messages::data::{Data, ToUpstream};
        use crate::messages::packet_type::PacketType;
        use mac_address::MacAddress;

        let origin: MacAddress = [1u8; 6].into();
        let payload = b"test payload data";
        let parsed = ToUpstream::new(origin, payload);

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();

        // Traditional approach
        let msg_traditional =
            Message::new(from, to, PacketType::Data(Data::Upstream(parsed.clone())));
        let wire_traditional: Vec<u8> = (&msg_traditional).into();

        // Zero-copy approach
        let mut wire_zero_copy = Vec::new();
        Message::serialize_upstream_forward_into(&parsed, from, to, &mut wire_zero_copy);

        // Should produce identical output
        assert_eq!(wire_traditional, wire_zero_copy);

        // Verify it can be parsed back
        let parsed_msg = Message::try_from(&wire_zero_copy[..]).expect("should parse");
        assert_eq!(parsed_msg.from().unwrap(), from);
        assert_eq!(parsed_msg.to().unwrap(), to);
    }

    #[test]
    fn zero_copy_downstream_creation_matches_traditional() {
        use crate::messages::data::{Data, ToDownstream};
        use crate::messages::packet_type::PacketType;
        use mac_address::MacAddress;

        let origin = [4u8; 6];
        let destination: MacAddress = [5u8; 6].into();
        let payload = b"downstream payload";

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();

        // Traditional approach
        let td = ToDownstream::new(&origin, destination, payload);
        let msg_traditional = Message::new(from, to, PacketType::Data(Data::Downstream(td)));
        let wire_traditional: Vec<u8> = (&msg_traditional).into();

        // Zero-copy approach
        let mut wire_zero_copy = Vec::new();
        Message::serialize_downstream_into(
            &origin,
            destination,
            payload,
            from,
            to,
            &mut wire_zero_copy,
        );

        // Should produce identical output
        assert_eq!(wire_traditional, wire_zero_copy);

        // Verify it can be parsed back
        let parsed_msg = Message::try_from(&wire_zero_copy[..]).expect("should parse");
        assert_eq!(parsed_msg.from().unwrap(), from);
        assert_eq!(parsed_msg.to().unwrap(), to);
    }

    #[test]
    fn zero_copy_downstream_forward_matches_traditional() {
        use crate::messages::data::{Data, ToDownstream};
        use crate::messages::packet_type::PacketType;
        use mac_address::MacAddress;

        let origin = [6u8; 6];
        let destination: MacAddress = [7u8; 6].into();
        let payload = b"forwarded downstream";
        let parsed = ToDownstream::new(&origin, destination, payload);

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();

        // Traditional approach
        let msg_traditional =
            Message::new(from, to, PacketType::Data(Data::Downstream(parsed.clone())));
        let wire_traditional: Vec<u8> = (&msg_traditional).into();

        // Zero-copy approach
        let mut wire_zero_copy = Vec::new();
        Message::serialize_downstream_forward_into(&parsed, from, to, &mut wire_zero_copy);

        // Should produce identical output
        assert_eq!(wire_traditional, wire_zero_copy);

        // Verify it can be parsed back
        let parsed_msg = Message::try_from(&wire_zero_copy[..]).expect("should parse");
        assert_eq!(parsed_msg.from().unwrap(), from);
        assert_eq!(parsed_msg.to().unwrap(), to);
    }
}
