pub mod client_cache;
pub mod node;
pub mod routing;

// Re-export shared modules from node_lib to avoid duplication
pub use node_lib::control::route;
pub use node_lib::control::routing_utils;

use crate::args::RsuArgs;
use anyhow::{anyhow, Result};
use client_cache::ClientCache;
use common::device::Device;
use common::network_interface::NetworkInterface;
use mac_address::MacAddress;
use node::ReplyType;
use node_lib::messages::{
    auth::Auth, control::Control, data::Data, message::Message, packet_type::PacketType,
};
use routing::Routing;
use server_lib::cloud_protocol::{
    CloudMessage, DownstreamForward, KeyExchangeForward, KeyExchangeResponse,
    SessionTerminatedForward, UpstreamForward,
};
use std::{
    io::IoSlice,
    net::SocketAddr,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::net::UdpSocket;
use tracing::Instrument;

// Re-export type aliases for cleaner code
use node_lib::{Shared, SharedDevice};

pub struct Rsu {
    args: RsuArgs,
    routing: Shared<Routing>,
    device: SharedDevice,
    cache: Arc<ClientCache>,
    /// UDP socket for communicating with the server.
    /// Always present; connected to the configured server address when `server_ip`
    /// is set, or to a no-op loopback address when unconfigured (test scenarios).
    cloud_socket: Arc<UdpSocket>,
    node_name: String,
}

impl Rsu {
    pub fn new(args: RsuArgs, device: Arc<Device>, node_name: String) -> Result<Arc<Self>> {
        // Create tracing span for this node's initialization
        let _span = tracing::info_span!("node", name = %node_name).entered();

        // Always create a cloud socket, connected to the configured server or a
        // no-op loopback address when server_ip is not set (test scenarios).
        // Use std::net::UdpSocket (synchronous) and convert to Tokio so this works
        // in both current-thread and multi-thread runtimes.
        let bind_addr = args
            .cloud_ip
            .map(|ip| format!("{}:0", ip))
            .unwrap_or_else(|| "0.0.0.0:0".to_string());
        let server_addr: SocketAddr = args
            .rsu_params
            .server_ip
            .map(|ip| format!("{}:{}", ip, args.rsu_params.server_port))
            .unwrap_or_else(|| "127.0.0.1:1".to_string())
            .parse()
            .map_err(|e| anyhow!("Invalid server address: {}", e))?;

        let std_sock = std::net::UdpSocket::bind(&bind_addr)
            .map_err(|e| anyhow!("Failed to bind cloud socket at {}: {}", bind_addr, e))?;
        std_sock
            .connect(server_addr)
            .map_err(|e| anyhow!("Failed to connect cloud socket to {}: {}", server_addr, e))?;
        std_sock
            .set_nonblocking(true)
            .map_err(|e| anyhow!("Failed to set cloud socket non-blocking: {}", e))?;
        let socket = UdpSocket::from_std(std_sock)
            .map_err(|e| anyhow!("Failed to register cloud socket with Tokio: {}", e))?;

        if args.rsu_params.server_ip.is_some() {
            tracing::info!(
                server = %server_addr,
                bind = %bind_addr,
                "RSU cloud socket created for server communication"
            );
        }

        let cloud_socket = Arc::new(socket);

        let rsu = Arc::new(Self {
            routing: Arc::new(RwLock::new(Routing::new(&args)?)),
            args,
            device,
            cache: ClientCache::default().into(),
            cloud_socket,
            node_name,
        });

        tracing::info!(
            bind = %rsu.args.bind,
            mac = %rsu.device.mac_address(),
            mtu = rsu.args.mtu,
            hello_history = rsu.args.rsu_params.hello_history,
            hello_period_ms = rsu.args.rsu_params.hello_periodicity,
            cached_candidates = rsu.args.rsu_params.cached_candidates,
            "RSU initialized"
        );
        rsu.hello_task()?;
        Self::wire_traffic_task(rsu.clone())?;
        rsu.registration_task()?;
        rsu.cloud_recv_task()?;
        Ok(rsu)
    }

    /// Get route to a specific MAC address. Used for testing latency measurement.
    pub fn get_route_to(&self, mac: MacAddress) -> Option<route::Route> {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .get_route_to(Some(mac))
    }

    /// Get count of next hops in routing table. Used for testing.
    pub fn next_hop_count(&self) -> usize {
        self.routing
            .read()
            .expect("routing table read lock poisoned")
            .iter_next_hops()
            .count()
    }

    /// Return this node's name.
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Return this node's VANET MAC address.
    pub fn mac_address(&self) -> MacAddress {
        self.device.mac_address()
    }

    /// Return all known OBU clients: `(obu_mac, via_mac)`.
    pub fn get_clients(&self) -> Vec<(MacAddress, MacAddress)> {
        self.cache
            .get_all_clients()
            .into_iter()
            .filter_map(|obu| self.cache.get(obu).map(|via| (obu, via)))
            .collect()
    }

    /// Return all known next-hop MACs and their best route info: `(mac, hops, latency_us)`.
    pub fn get_next_hops_info(&self) -> Vec<(MacAddress, u32, Option<u64>)> {
        let routing = self
            .routing
            .read()
            .expect("routing table read lock poisoned");
        routing
            .iter_next_hops()
            .filter_map(|mac| {
                routing
                    .get_route_to(Some(*mac))
                    .map(|r| (*mac, r.hops, r.latency.map(|d| d.as_micros() as u64)))
            })
            .collect()
    }

    fn wire_traffic_task(rsu: Arc<Self>) -> Result<()> {
        let device = rsu.device.clone();
        let node_name = rsu.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let rsu = rsu.clone();
                    let messages = node::wire_traffic(&device, |pkt, size| async move {
                        let data = &pkt[..size];
                        let mut all_responses = Vec::with_capacity(4);
                        let mut offset = 0;

                        while offset < data.len() {
                            match Message::try_from(&data[offset..]) {
                                Ok(msg) => {
                                    let response = rsu.handle_msg(&msg).await;

                                    if let Ok(Some(responses)) = response {
                                        all_responses.extend(responses);
                                    }
                                    offset += msg.wire_size();
                                }
                                Err(e) => {
                                    tracing::trace!(
                                        offset = offset,
                                        remaining = data.len() - offset,
                                        total_size = data.len(),
                                        error = %e,
                                        "Failed to parse message, stopping batch processing"
                                    );
                                    break;
                                }
                            }
                        }

                        if all_responses.is_empty() {
                            Ok(None)
                        } else {
                            Ok(Some(all_responses))
                        }
                    })
                    .await;

                    match messages {
                        Ok(Some(messages)) => {
                            // RSU only produces WireFlat replies now (no TAP)
                            let _ = node::handle_messages_wire_only(messages, &device).await;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::error!(error = %e, "Wire traffic processing error");
                        }
                    }
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    async fn handle_msg(&self, msg: &Message<'_>) -> Result<Option<Vec<ReplyType>>> {
        match msg.get_packet_type() {
            PacketType::Data(Data::Upstream(buf)) => {
                // RSU is now a forwarding relay: pass upstream data to the server
                // opaquely (without decryption). The server handles all crypto.
                let source: [u8; 6] = buf
                    .source()
                    .get(0..6)
                    .ok_or_else(|| anyhow!("message source too short"))?
                    .try_into()?;
                let source: MacAddress = source.into();

                // Update client cache so we know which OBU is reachable via which neighbor
                self.cache.store_mac(source, msg.from().unwrap_or(source));

                if self.args.rsu_params.server_ip.is_some() {
                    let fwd = UpstreamForward::new(
                        self.device.mac_address(),
                        source,
                        buf.data().to_vec(),
                    );
                    if let Err(e) = self.cloud_socket.send(&fwd.to_bytes()).await {
                        tracing::warn!(error = %e, "Failed to forward upstream to server");
                    }
                } else {
                    tracing::trace!("No server configured, dropping upstream data");
                }

                Ok(None)
            }
            PacketType::Control(Control::HeartbeatReply(hbr)) => {
                if hbr.source() == self.device.mac_address() {
                    self.routing
                        .write()
                        .expect("routing table write lock poisoned during heartbeat reply")
                        .handle_heartbeat_reply(msg, self.device.mac_address())
                } else {
                    Ok(None)
                }
            }
            PacketType::Auth(Auth::KeyExchangeInit(ke_init)) => {
                // Relay KeyExchangeInit from OBU to server via cloud protocol.
                // Use the originating OBU MAC from the payload (ke_init.sender())
                // as the authoritative identifier, since KeyExchangeInit may be
                // forwarded hop-by-hop and msg.from() only reflects the last hop.
                if self.args.rsu_params.server_ip.is_some() {
                    let obu_mac = ke_init.sender();

                    // Optionally use the VANET frame source only for diagnostics
                    // and anti-spoofing heuristics, but do not change the
                    // authoritative OBU MAC or drop the message based on it.
                    match msg.from() {
                        Ok(from_mac) => {
                            if from_mac != obu_mac {
                                // Normal in multi-hop mesh: the immediate sender is a relay OBU
                                // whose MAC differs from the originating OBU in the payload.
                                tracing::debug!(
                                    relay = %from_mac,
                                    obu = %obu_mac,
                                    "KeyExchangeInit relayed via intermediate OBU"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                obu = %obu_mac,
                                "Failed to parse source MAC in KeyExchangeInit frame; proceeding with payload sender()"
                            );
                        }
                    }
                    let ke_bytes: Vec<u8> = ke_init.into();
                    let fwd =
                        match KeyExchangeForward::new(obu_mac, self.device.mac_address(), ke_bytes)
                        {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    obu = %obu_mac,
                                    "Failed to build KeyExchangeForward"
                                );
                                return Ok(None);
                            }
                        };
                    // Cache the OBU's reachability via the last-hop sender so that
                    // handle_key_exchange_response can route the reply back even
                    // when the heartbeat-based routing table has no entry for this
                    // OBU yet (e.g., on first connection or after a range change).
                    if let Ok(from_mac) = msg.from() {
                        self.cache.store_mac(obu_mac, from_mac);
                    }
                    if let Err(e) = self.cloud_socket.send(&fwd.to_bytes()).await {
                        tracing::warn!(
                            error = %e,
                            obu = %obu_mac,
                            "Failed to forward KeyExchangeInit to server"
                        );
                    } else {
                        tracing::info!(
                            obu = %obu_mac,
                            via_rsu = %self.device.mac_address(),
                            "Relayed KeyExchangeInit to server"
                        );
                    }
                }
                Ok(None)
            }
            PacketType::Data(Data::Downstream(_))
            | PacketType::Control(Control::Heartbeat(_))
            | PacketType::Auth(Auth::KeyExchangeReply(_))
            // SessionTerminated is only delivered to OBUs; the RSU forwards it via
            // handle_session_terminated_forward() when received from the server cloud
            // socket, so if it arrives on the VANET wire here it's already been handled.
            | PacketType::Auth(Auth::SessionTerminated(_)) => Ok(None),
        }
    }

    fn hello_task(&self) -> Result<()> {
        let periodicity = self.args.rsu_params.hello_periodicity;
        let periodicity = Duration::from_millis(periodicity.into());
        let routing = self.routing.clone();
        let device = self.device.clone();
        let node_name = self.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let msg: Vec<u8> = {
                        let mut routing = routing
                            .write()
                            .expect("routing table write lock poisoned in heartbeat task");
                        let msg = routing.send_heartbeat(device.mac_address());
                        (&msg).into()
                    };
                    let vec = [IoSlice::new(&msg)];
                    match device.send_vectored(&vec).await {
                        Ok(n) => {
                            tracing::trace!(bytes_sent = n, "Heartbeat sent");
                        }
                        Err(e) => {
                            tracing::error!(
                                error = %e,
                                size = msg.len(),
                                "Failed to send heartbeat"
                            );
                        }
                    }
                    let jitter: u32 =
                        rand::random::<u32>() % (periodicity.as_millis() as f32 / 10.0) as u32;
                    let jitter = Duration::from_millis(jitter.into());
                    let sleep_duration = periodicity + jitter;
                    let _ = tokio_timerfd::sleep(sleep_duration).await;
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    /// Periodically send a `RegistrationMessage` to the configured server.
    ///
    /// Each message contains this RSU's VANET MAC and the list of OBU MACs
    /// currently held in the client cache.  The task fires every
    /// `hello_periodicity` ms (re-using the same interval as heartbeats).
    ///
    /// Does nothing if no `server_ip` was configured.
    fn registration_task(&self) -> Result<()> {
        if self.args.rsu_params.server_ip.is_none() {
            return Ok(());
        }
        let socket = self.cloud_socket.clone();

        let cache = self.cache.clone();
        let rsu_mac = self.device.mac_address();
        let periodicity = Duration::from_millis(u64::from(self.args.rsu_params.hello_periodicity));
        let node_name = self.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                loop {
                    let obu_macs = cache.get_all_clients();
                    let msg = server_lib::RegistrationMessage::new(rsu_mac, obu_macs.clone());
                    let bytes = msg.to_bytes();

                    match socket.send(&bytes).await {
                        Ok(_) => {
                            tracing::debug!(
                                obu_count = obu_macs.len(),
                                "Sent RSU registration to server"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Failed to send registration to server");
                        }
                    }

                    tokio_timerfd::sleep(periodicity).await.ok();
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    /// Receive downstream data from the server and forward to OBUs via VANET.
    ///
    /// Listens on the connected cloud socket for `DownstreamForward` messages.
    /// For each message, looks up the route to the destination OBU and constructs
    /// a VANET Downstream message to deliver it.
    ///
    /// Does nothing if no `server_ip` was configured.
    fn cloud_recv_task(&self) -> Result<()> {
        if self.args.rsu_params.server_ip.is_none() {
            return Ok(());
        }
        let socket = self.cloud_socket.clone();

        let device = self.device.clone();
        let routing = self.routing.clone();
        let cache = self.cache.clone();
        let node_name = self.node_name.clone();

        let span = tracing::info_span!("node", name = %node_name);
        tokio::task::spawn(
            async move {
                let mut buf = vec![0u8; 65536];
                loop {
                    match socket.recv(&mut buf).await {
                        Ok(len) => {
                            let data = &buf[..len];
                            match CloudMessage::try_from_bytes(data) {
                                Some(CloudMessage::DownstreamForward(fwd)) => {
                                    Self::handle_downstream_forward(
                                        &fwd, &device, &routing, &cache,
                                    )
                                    .await;
                                }
                                Some(CloudMessage::KeyExchangeResponse(rsp)) => {
                                    Self::handle_key_exchange_response(
                                        &rsp, &device, &routing, &cache,
                                    )
                                    .await;
                                }
                                Some(CloudMessage::SessionTerminatedForward(stf)) => {
                                    Self::handle_session_terminated_forward(
                                        &stf, &device, &routing, &cache,
                                    )
                                    .await;
                                }
                                Some(_) => {
                                    tracing::trace!(
                                        "Received non-downstream cloud message, ignoring"
                                    );
                                }
                                None => {
                                    tracing::debug!(
                                        len = len,
                                        "Received unrecognized data from server"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "Error receiving from cloud socket");
                        }
                    }
                }
            }
            .instrument(span),
        );
        Ok(())
    }

    /// Handle a DownstreamForward message from the server.
    ///
    /// Looks up the route to the destination OBU and sends a VANET Downstream
    /// message via the device.
    async fn handle_downstream_forward(
        fwd: &DownstreamForward,
        device: &Arc<Device>,
        routing: &Shared<Routing>,
        cache: &Arc<ClientCache>,
    ) {
        let dest_mac = fwd.obu_dest_mac;
        let origin_mac = fwd.origin_mac;

        // Try to find the next hop to the destination OBU
        let next_hop = {
            let routing = routing
                .read()
                .expect("routing table read lock poisoned during downstream forward");

            // First try: direct route from routing table
            if let Some(route) = routing.get_route_to(Some(dest_mac)) {
                Some(route.mac)
            } else {
                // Second try: check client cache for the OBU's neighbor
                cache.get(dest_mac)
            }
        };

        let Some(next_hop) = next_hop else {
            tracing::debug!(
                dest = %dest_mac,
                "No route to destination OBU for downstream forward"
            );
            return;
        };

        // Build VANET Downstream message and send via device
        let mut wire = Vec::with_capacity(30 + fwd.payload.len());
        Message::serialize_downstream_into(
            &origin_mac.bytes(),
            dest_mac,
            &fwd.payload,
            device.mac_address(),
            next_hop,
            &mut wire,
        );

        let slices = [IoSlice::new(&wire)];
        if let Err(e) = device.send_vectored(&slices).await {
            tracing::error!(
                error = %e,
                dest = %dest_mac,
                "Failed to send downstream forward to VANET"
            );
        }
    }

    /// Handle a KeyExchangeResponse from the server and relay to OBU on VANET.
    async fn handle_key_exchange_response(
        rsp: &KeyExchangeResponse,
        device: &Arc<Device>,
        routing: &Shared<Routing>,
        cache: &Arc<ClientCache>,
    ) {
        let dest_mac = rsp.obu_dest_mac;

        // Parse the key exchange reply payload
        let ke_reply = match node_lib::messages::auth::key_exchange::KeyExchangeReply::try_from(
            rsp.payload.as_slice(),
        ) {
            Ok(reply) => reply,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    obu = %dest_mac,
                    "Failed to parse KeyExchangeReply payload from server"
                );
                return;
            }
        };

        // Find the next hop to the destination OBU.
        // Prefer the ClientCache: it was set when the KeyExchangeInit arrived and
        // always reflects the actual forwarding path (e.g. OBU1→OBU2→RSU). The
        // routing table may hold a stale direct-route entry that skips intermediate
        // hops, causing the reply to be misdelivered.
        let next_hop = {
            let from_cache = cache.get(dest_mac);
            if from_cache.is_some() {
                from_cache
            } else {
                let routing = routing
                    .read()
                    .expect("routing table read lock poisoned during key exchange response");
                routing.get_route_to(Some(dest_mac)).map(|r| r.mac)
            }
        };

        let Some(next_hop) = next_hop else {
            tracing::warn!(
                dest = %dest_mac,
                "No route/cache entry to OBU for key exchange response relay — reply dropped"
            );
            return;
        };

        // Construct VANET KeyExchangeReply message and send
        let key_id = ke_reply.key_id();
        let msg = Message::new(
            device.mac_address(),
            next_hop,
            PacketType::Auth(Auth::KeyExchangeReply(ke_reply)),
        );
        let wire: Vec<u8> = (&msg).into();
        let slices = [IoSlice::new(&wire)];
        if let Err(e) = device.send_vectored(&slices).await {
            tracing::error!(
                error = %e,
                dest = %dest_mac,
                "Failed to relay KeyExchangeReply to OBU on VANET"
            );
        } else {
            tracing::info!(
                dest = %dest_mac,
                next_hop = %next_hop,
                key_id = key_id,
                "Relayed KeyExchangeReply from server to OBU on VANET"
            );
        }
    }

    /// Handle a `SessionTerminatedForward` from the server and relay as a VANET
    /// `SessionTerminated` control message to the target OBU.
    async fn handle_session_terminated_forward(
        stf: &SessionTerminatedForward,
        device: &Arc<Device>,
        routing: &Shared<Routing>,
        cache: &Arc<ClientCache>,
    ) {
        let dest_mac = stf.obu_mac;

        // Find the next hop to the target OBU
        let next_hop = {
            let routing = routing
                .read()
                .expect("routing table read lock poisoned during session terminated forward");
            if let Some(route) = routing.get_route_to(Some(dest_mac)) {
                Some(route.mac)
            } else {
                cache.get(dest_mac)
            }
        };

        let Some(next_hop) = next_hop else {
            tracing::debug!(
                dest = %dest_mac,
                "No route to OBU for session terminated relay"
            );
            return;
        };

        use node_lib::messages::auth::session_terminated::SessionTerminated;
        // Pass timestamp, nonce, and signature through transparently so the OBU
        // can authenticate and replay-check the revocation notice.
        let st = match (
            stf.timestamp_secs,
            stf.nonce,
            stf.sig_algo,
            stf.signature.as_deref(),
        ) {
            (Some(ts), Some(nonce), Some(algo), Some(sig)) => {
                SessionTerminated::new_signed(dest_mac, ts, nonce, algo, sig.to_vec())
            }
            _ => SessionTerminated::new(dest_mac),
        };
        let msg = Message::new(
            device.mac_address(),
            next_hop,
            PacketType::Auth(Auth::SessionTerminated(st)),
        );
        let wire: Vec<u8> = (&msg).into();
        let slices = [IoSlice::new(&wire)];
        if let Err(e) = device.send_vectored(&slices).await {
            tracing::error!(
                error = %e,
                dest = %dest_mac,
                "Failed to relay SessionTerminated to OBU on VANET"
            );
        } else {
            tracing::info!(
                dest = %dest_mac,
                "Relayed SessionTerminated from server to OBU"
            );
        }
    }
}

#[cfg(test)]
pub(crate) fn handle_msg_for_test(
    routing: std::sync::Arc<std::sync::RwLock<Routing>>,
    device_mac: mac_address::MacAddress,
    cache: std::sync::Arc<ClientCache>,
    msg: &node_lib::messages::message::Message<'_>,
) -> anyhow::Result<Option<Vec<ReplyType>>> {
    use node_lib::messages::{auth::Auth, control::Control, data::Data, packet_type::PacketType};

    match msg.get_packet_type() {
        PacketType::Data(Data::Upstream(buf)) => {
            // In the new architecture, the RSU forwards upstream data to the server.
            // For testing without a real cloud socket, we just record the forwarding info.
            let source: [u8; 6] = buf
                .source()
                .get(0..6)
                .ok_or_else(|| anyhow::anyhow!("error"))?
                .try_into()?;
            let source: mac_address::MacAddress = source.into();
            cache.store_mac(source, msg.from().unwrap_or(source));

            // Return the raw upstream data as a WireFlat to indicate it would be forwarded
            // to the server (simulates the UpstreamForward cloud message)
            let fwd = UpstreamForward::new(device_mac, source, buf.data().to_vec());
            Ok(Some(vec![ReplyType::WireFlat(fwd.to_bytes())]))
        }
        PacketType::Control(Control::HeartbeatReply(hbr)) => {
            if hbr.source() == device_mac {
                routing
                    .write()
                    .unwrap()
                    .handle_heartbeat_reply(msg, device_mac)
            } else {
                Ok(None)
            }
        }
        PacketType::Data(Data::Downstream(_))
        | PacketType::Control(Control::Heartbeat(_))
        | PacketType::Auth(Auth::KeyExchangeInit(_))
        | PacketType::Auth(Auth::KeyExchangeReply(_))
        | PacketType::Auth(Auth::SessionTerminated(_)) => Ok(None),
    }
}

#[cfg(test)]
mod rsu_tests {
    use super::{handle_msg_for_test, routing::Routing, ClientCache, ReplyType};
    use mac_address::MacAddress;
    use node_lib::messages::control::Control;
    use node_lib::messages::{
        data::{Data, ToUpstream},
        message::Message,
        packet_type::PacketType,
    };

    #[test]
    fn upstream_broadcast_forwards_to_server() {
        let args = crate::args::RsuArgs {
            bind: String::new(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

        let from_mac: MacAddress = [1u8; 6].into();
        let dest_bytes = [255u8; 6];
        let payload = [0u8; 4];
        // inner data is: dest(6) + from(6) + payload
        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_bytes);
        inner.extend_from_slice(&from_mac.bytes());
        inner.extend_from_slice(&payload);
        let tu = ToUpstream::new(from_mac, &inner);
        let msg = Message::new(
            from_mac,
            dest_bytes.into(),
            PacketType::Data(Data::Upstream(tu)),
        );

        let res =
            handle_msg_for_test(routing.clone(), [9u8; 6].into(), cache.clone(), &msg).expect("ok");
        assert!(res.is_some());
        let v = res.unwrap();
        // Should have a WireFlat entry (the UpstreamForward to server)
        assert!(!v.is_empty());
        assert!(matches!(&v[0], ReplyType::WireFlat(_)));
    }

    #[test]
    fn heartbeat_reply_for_other_source_returns_none() {
        let args = crate::args::RsuArgs {
            bind: String::new(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

        // Build a Heartbeat/Reply with a source different from RSU device_mac
        let src: MacAddress = [1u8; 6].into();
        let hb = node_lib::messages::control::heartbeat::Heartbeat::new(
            std::time::Duration::from_millis(0),
            0u32,
            src,
        );
        let reply_sender: MacAddress = [2u8; 6].into();
        let hbr =
            node_lib::messages::control::heartbeat::HeartbeatReply::from_sender(&hb, reply_sender);
        let msg = Message::new(
            [3u8; 6].into(),
            [255u8; 6].into(),
            PacketType::Control(Control::HeartbeatReply(hbr)),
        );

        // Device mac differs from hbr.source(); should return Ok(None)
        let res = handle_msg_for_test(routing, [9u8; 6].into(), cache, &msg).expect("ok");
        assert!(res.is_none());
    }

    #[test]
    fn upstream_unicast_forwards_to_server() {
        let args = crate::args::RsuArgs {
            bind: String::new(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: crate::args::RsuParameters {
                hello_history: 2,
                hello_periodicity: 5000,
                cached_candidates: 3,
                server_ip: None,
                server_port: 8080,
            },
        };
        let routing = std::sync::Arc::new(std::sync::RwLock::new(
            Routing::new(&args).expect("routing"),
        ));
        let cache = std::sync::Arc::new(ClientCache::default());

        let device_mac: MacAddress = [9u8; 6].into();
        let dest_client: MacAddress = [10u8; 6].into();
        let from_client: MacAddress = [1u8; 6].into();

        let mut inner = Vec::new();
        inner.extend_from_slice(&dest_client.bytes());
        inner.extend_from_slice(&from_client.bytes());
        inner.extend_from_slice(&[0u8; 8]);
        let tu = ToUpstream::new(from_client, &inner);
        let msg = Message::new(
            from_client,
            dest_client,
            PacketType::Data(Data::Upstream(tu)),
        );

        let res =
            handle_msg_for_test(routing.clone(), device_mac, cache.clone(), &msg).expect("ok");
        assert!(res.is_some());
        let msgs = res.unwrap();
        // Should produce a WireFlat (UpstreamForward to server)
        assert_eq!(msgs.len(), 1);
        assert!(matches!(&msgs[0], ReplyType::WireFlat(_)));

        // Verify it's a valid UpstreamForward
        if let ReplyType::WireFlat(bytes) = &msgs[0] {
            let parsed =
                server_lib::UpstreamForward::try_from_bytes(bytes).expect("valid upstream forward");
            assert_eq!(parsed.rsu_mac, device_mac);
            assert_eq!(parsed.obu_source_mac, from_client);
        }
    }
}
