use anyhow::Result;
use common::device::Device;
use common::network_interface::NetworkInterface;
use mac_address::MacAddress;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio::time;

/// Represents a packet in the device bridge
#[derive(Clone)]
pub struct BridgePacket {
    pub data: Vec<u8>,
    pub timestamp: Instant,
    pub sender_mac: MacAddress,
}

/// A bridge that forwards device traffic between network namespaces
/// by reading from each device and broadcasting to all other devices
pub struct DeviceBridge {
    devices: HashMap<String, Arc<Device>>,
    forwarding_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl DeviceBridge {
    pub fn new(devices: HashMap<String, Arc<Device>>) -> Self {
        Self {
            devices,
            forwarding_handles: Vec::new(),
        }
    }
    
    /// Start the device bridge with the given latency settings between nodes
    pub async fn start(&mut self, latency_map: HashMap<String, HashMap<String, Duration>>) -> Result<()> {
        // Create a broadcast channel for all device traffic
        let (tx, _) = tokio::sync::broadcast::channel::<(String, BridgePacket)>(1024);
        
        // For each device, create a forwarding task
        for (node_name, device) in &self.devices {
            let device_clone = device.clone();
            let tx_clone = tx.clone();
            let node_name_clone = node_name.clone();
            let devices_clone = self.devices.clone();
            let latency_map_clone = latency_map.clone();
            
            // Create a receiver for this specific device
            let rx = tx.subscribe();
            
            let handle = tokio::spawn(async move {
                Self::device_forwarding_task(
                    node_name_clone,
                    device_clone,
                    tx_clone,
                    rx,
                    devices_clone,
                    latency_map_clone,
                ).await;
            });
            
            self.forwarding_handles.push(handle);
        }
        
        Ok(())
    }
    
    async fn device_forwarding_task(
        node_name: String,
        device: Arc<Device>,
        tx: tokio::sync::broadcast::Sender<(String, BridgePacket)>,
        mut rx: tokio::sync::broadcast::Receiver<(String, BridgePacket)>,
        _devices: HashMap<String, Arc<Device>>,
        latency_map: HashMap<String, HashMap<String, Duration>>,
    ) {
        let mut read_buffer = vec![0u8; 2048];
        
        loop {
            tokio::select! {
                // Read from this device and broadcast to others
                read_result = device.recv(&mut read_buffer) => {
                    match read_result {
                        Ok(n) if n > 0 => {
                            let packet = BridgePacket {
                                data: read_buffer[..n].to_vec(),
                                timestamp: Instant::now(),
                                sender_mac: device.mac_address(),
                            };
                            
                            tracing::debug!(
                                "Device bridge: {} sent {} bytes: {}",
                                node_name,
                                n,
                                packet.data.iter().take(32).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
                            );
                            
                            // Broadcast to all other devices (excluding sender)
                            if let Err(e) = tx.send((node_name.clone(), packet)) {
                                tracing::error!("Device bridge failed to broadcast packet from {}: {}", node_name, e);
                                break;
                            }
                        }
                        Ok(_) => {
                            // Empty read, continue
                        }
                        Err(e) => {
                            // Only log serious errors, not temporary ones like EAGAIN
                            if !matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) {
                                tracing::debug!("Device bridge read error from {}: {}", node_name, e);
                            }
                        }
                    }
                }
                
                // Receive packets from other devices and forward to this device
                broadcast_result = rx.recv() => {
                    match broadcast_result {
                        Ok((sender_node, packet)) => {
                            // Don't forward back to sender
                            if sender_node == node_name {
                                continue;
                            }
                            
                            // Apply latency if configured
                            let latency = latency_map
                                .get(&sender_node)
                                .and_then(|targets| targets.get(&node_name))
                                .copied()
                                .unwrap_or(Duration::ZERO);
                            
                            if !latency.is_zero() {
                                let target_time = packet.timestamp + latency;
                                let now = Instant::now();
                                if target_time > now {
                                    time::sleep_until(time::Instant::from_std(target_time)).await;
                                }
                            }
                            
                            // Forward to this device
                            let slices = vec![std::io::IoSlice::new(&packet.data)];
                            match device.send_vectored(&slices).await {
                                Ok(bytes_sent) => {
                                    tracing::debug!(
                                        "Device bridge: forwarded {} bytes from {} to {}",
                                        bytes_sent, sender_node, node_name
                                    );
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "Device bridge: failed to forward from {} to {}: {}",
                                        sender_node, node_name, e
                                    );
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::info!("Device bridge broadcast channel closed for {}", node_name);
                            break;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Device bridge lagged {} messages for {}", n, node_name);
                            continue;
                        }
                    }
                }
                
                // Yield periodically to prevent busy-waiting
                _ = time::sleep(Duration::from_micros(100)) => {
                    // Small yield to prevent excessive CPU usage
                }
            }
        }
        
        tracing::info!("Device bridge task for {} terminated", node_name);
    }
}

impl Drop for DeviceBridge {
    fn drop(&mut self) {
        // Abort all forwarding tasks
        for handle in &self.forwarding_handles {
            handle.abort();
        }
    }
}