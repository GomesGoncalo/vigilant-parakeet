use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use common::device::{Device, DeviceIo};
use mac_address::MacAddress;
use std::os::unix::io::FromRawFd;
use tokio::io::unix::AsyncFd;

/// A device hub that enables cross-namespace communication between devices
/// by bridging their traffic through socketpairs
pub struct DeviceHub {
    /// Maps device names to their hub connection file descriptors
    hub_connections: Arc<Mutex<HashMap<String, i32>>>,
    /// The hub task that forwards traffic between devices
    _hub_task: tokio::task::JoinHandle<()>,
}

impl DeviceHub {
    /// Create a new device hub with the given device names
    pub async fn new(device_names: Vec<String>) -> Result<(Self, HashMap<String, Arc<Device>>)> {
        let mut devices = HashMap::new();
        let mut hub_fds = Vec::new();
        let mut node_fds = Vec::new();
        let hub_connections = Arc::new(Mutex::new(HashMap::new()));
        
        // Create socketpairs for each device to connect to the hub
        for device_name in &device_names {
            let mut fds = [0; 2];
            unsafe {
                let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr());
                if r != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                // Make both ends non-blocking
                if libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK) != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                if libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) != 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            
            let node_fd = fds[0];  // Device uses this
            let hub_fd = fds[1];   // Hub uses this
            
            // Create device using the node end of the socketpair
            let mac: MacAddress = [0x52, 0x54, 0x00, 
                                   (device_names.len() as u8).wrapping_add(1),
                                   (device_name.len() as u8),
                                   device_name.bytes().fold(0u8, |acc, b| acc.wrapping_add(b))].into();
            
            let async_fd = AsyncFd::new(unsafe { DeviceIo::from_raw_fd(node_fd) })?;
            let device = Device::from_asyncfd_for_bench(mac, async_fd);
            
            devices.insert(device_name.clone(), Arc::new(device));
            hub_fds.push(hub_fd);
            node_fds.push(node_fd);
            
            {
                let mut connections = hub_connections.lock().await;
                connections.insert(device_name.clone(), hub_fd);
            }
        }
        
        // Start the hub task
        let hub_connections_clone = hub_connections.clone();
        let hub_task = tokio::spawn(async move {
            Self::run_hub(hub_fds, device_names, hub_connections_clone).await;
        });
        
        Ok((Self {
            hub_connections,
            _hub_task: hub_task,
        }, devices))
    }
    
    async fn run_hub(
        hub_fds: Vec<i32>,
        device_names: Vec<String>,
        _hub_connections: Arc<Mutex<HashMap<String, i32>>>,
    ) {
        tracing::info!("Starting device hub with {} devices: {:?}", hub_fds.len(), device_names);
        
        let mut buf = vec![0u8; 2048];
        
        loop {
            for (i, &hub_fd) in hub_fds.iter().enumerate() {
                // Try to read from this device's hub connection
                let n = unsafe {
                    libc::recv(
                        hub_fd,
                        buf.as_mut_ptr() as *mut _,
                        buf.len(),
                        libc::MSG_DONTWAIT,
                    )
                };
                
                if n > 0 {
                    let n = n as usize;
                    let data = &buf[..n];
                    
                    tracing::debug!(
                        "Device hub: received {} bytes from device {}: {}",
                        n,
                        device_names[i],
                        data.iter().take(32).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
                    );
                    
                    // Forward to all other devices
                    for (j, &target_fd) in hub_fds.iter().enumerate() {
                        if j != i {  // Don't send back to sender
                            let sent = unsafe {
                                libc::send(target_fd, data.as_ptr() as *const _, data.len(), libc::MSG_DONTWAIT)
                            };
                            
                            if sent > 0 {
                                tracing::debug!(
                                    "Device hub: forwarded {} bytes from {} to {}",
                                    sent, device_names[i], device_names[j]
                                );
                            } else if sent < 0 {
                                let errno = unsafe { *libc::__errno_location() };
                                if errno != libc::EAGAIN && errno != libc::EWOULDBLOCK {
                                    tracing::debug!(
                                        "Device hub: failed to forward from {} to {}: errno {}",
                                        device_names[i], device_names[j], errno
                                    );
                                }
                            }
                        }
                    }
                }
            }
            
            // Small yield to prevent busy-waiting
            tokio::time::sleep(std::time::Duration::from_micros(100)).await;
        }
    }
}