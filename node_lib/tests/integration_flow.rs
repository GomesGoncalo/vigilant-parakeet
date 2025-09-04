use mac_address::MacAddress;
use node_lib::control::node::{handle_messages, ReplyType};
use node_lib::test_helpers::util::{mk_device_from_fd, mk_shim_pair};
use std::sync::Arc;

#[tokio::test]
async fn handle_messages_sends_to_tun_and_device() -> anyhow::Result<()> {
    let (tun, _peer) = mk_shim_pair();

    // create a pipe to stand in for device fd and make writer non-blocking
    let (reader_fd, writer_fd) = node_lib::test_helpers::util::mk_pipe_nonblocking()?;

    let mac: MacAddress = [1u8; 6].into();
    let device = mk_device_from_fd(mac, writer_fd);

    let tun = Arc::new(tun);
    let device = Arc::new(device);

    let tap = ReplyType::Tap(vec![vec![1u8, 2u8, 3u8]]);
    let wire = ReplyType::Wire(vec![vec![0u8; 14]]);

    let msgs = vec![tap, wire];
    handle_messages(msgs, &tun, &device, None).await?;

    // drain the reader side of the pipe to observe bytes written
    let mut buf = [0u8; 64];
    let n = node_lib::test_helpers::util::read_fd(reader_fd, &mut buf)?;
    node_lib::test_helpers::util::close_fd(reader_fd)?;
    node_lib::test_helpers::util::close_fd(writer_fd)?;
    assert!(n > 0);

    Ok(())
}
