use anyhow::Result;
use common::device::Device;
use std::{io::IoSlice, sync::Arc};

// Re-export shared types and functions from node_lib to avoid duplication
pub use node_lib::control::node::{buffer, bytes_to_hex, handle_messages, wire_traffic, ReplyType};

#[cfg(any(test, feature = "test_helpers"))]
pub use node_lib::control::node::{get_msgs, DebugReplyType};

/// Send only wire (device) messages, ignoring any TapFlat replies.
///
/// RSU no longer has a TAP device, so we only forward wire messages.
pub async fn handle_messages_wire_only(messages: Vec<ReplyType>, dev: &Arc<Device>) -> Result<()> {
    let wire_packets: Vec<Vec<u8>> = messages
        .into_iter()
        .filter_map(|reply| match reply {
            ReplyType::WireFlat(buf) => Some(buf),
            ReplyType::TapFlat(_) => None,
        })
        .collect();

    if !wire_packets.is_empty() {
        let slices: Vec<IoSlice> = wire_packets.iter().map(|p| IoSlice::new(p)).collect();
        dev.send_vectored(&slices).await.inspect_err(|e| {
            tracing::error!(
                error = %e,
                packet_count = wire_packets.len(),
                "Failed to batch send to device"
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{get_msgs, DebugReplyType, ReplyType};
    use anyhow::Result;
    use node_lib::messages::message::Message;

    #[test]
    fn get_msgs_ok_none() {
        let res: Result<Option<Vec<ReplyType>>> = Ok(None);
        let out = get_msgs(&res).expect("ok none");
        assert!(out.is_none());
    }

    #[test]
    fn get_msgs_ok_some_with_unparsable_wire() {
        // ReplyType::WireFlat with random bytes that won't parse to Message -> filtered out
        let replies = vec![ReplyType::WireFlat(vec![0u8; 3])];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        // should filter out unparsable wire entries
        assert!(dbg.is_empty());
    }

    #[test]
    fn get_msgs_ok_some_with_parsable_wire() {
        use mac_address::MacAddress;
        use node_lib::messages::data::Data;
        use node_lib::messages::data::ToUpstream;
        use node_lib::messages::packet_type::PacketType;

        let from: MacAddress = [2u8; 6].into();
        let to: MacAddress = [3u8; 6].into();
        let payload = b"hi";
        let tu = ToUpstream::new(from, payload);
        let data = Data::Upstream(tu);
        let pkt = PacketType::Data(data);
        let message = Message::new(from, to, pkt);

        let wire: Vec<u8> = (&message).into();
        let replies = vec![ReplyType::WireFlat(wire)];
        let res: Result<Option<Vec<ReplyType>>> = Ok(Some(replies));
        let dbg = get_msgs(&res).expect("ok some").expect("some");
        // should contain one WireFlat debug entry
        assert_eq!(dbg.len(), 1);
        match &dbg[0] {
            DebugReplyType::Wire(s) => {
                assert!(s.contains("Message"));
            }
            _ => panic!("expected WireFlat debug string"),
        }
    }
}
