//! Interface builder for creating network interfaces
//!
//! This module provides a unified interface for creating TAP/TUN devices
//! with consistent handling of both real and test implementations.

use anyhow::Result;
use common::tun::Tun;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Kernel transmit queue depth for simulator TAP devices.
///
/// Linux default is 1000 packets.  With a 9000 B VANET MTU and 119 nodes that
/// is ~1 GB of kernel sk_buff memory; shrinking to 16 drops it to ~17 MB.
/// Cloud/virtual interfaces (1500 B MTU) benefit proportionally.
const TAP_TXQUEUELEN: u32 = 16;

/// Builder for creating network interfaces with optional configuration
///
/// # Example
/// ```no_run
/// use simulator::interface_builder::InterfaceBuilder;
/// use std::net::Ipv4Addr;
///
/// # fn example() -> anyhow::Result<()> {
/// // Create a simple TAP interface
/// let vanet = InterfaceBuilder::new("vanet").build_tap()?;
///
/// // Create a TAP interface with IP and MTU
/// let virtual_tap = InterfaceBuilder::new("virtual")
///     .with_ip("10.0.0.1".parse()?)
///     .with_mtu(1400)
///     .build_tap()?;
/// # Ok(())
/// # }
/// ```
pub struct InterfaceBuilder {
    name: String,
    ip: Option<Ipv4Addr>,
    mtu: Option<u16>,
    netmask: Option<Ipv4Addr>,
    mac: Option<[u8; 6]>,
}

impl InterfaceBuilder {
    /// Create a new interface builder with the specified name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ip: None,
            mtu: None,
            netmask: None,
            mac: None,
        }
    }

    /// Set the IP address for this interface
    pub fn with_ip(mut self, ip: Ipv4Addr) -> Self {
        self.ip = Some(ip);
        self
    }

    /// Set the MTU for this interface
    pub fn with_mtu(mut self, mtu: u16) -> Self {
        self.mtu = Some(mtu);
        self
    }

    /// Set a fixed MAC address for this interface.
    ///
    /// The MAC is applied via `ip link set <name> address <mac> up` after the TAP
    /// device is created.  This is required when using the PKI allowlist, because
    /// the allowlist maps MAC addresses to expected signing keys — the MAC must be
    /// stable and known at configuration time.
    pub fn with_mac(mut self, mac: [u8; 6]) -> Self {
        self.mac = Some(mac);
        self
    }

    /// Set the subnet mask for this interface (e.g. `255.255.255.0` for /24).
    ///
    /// Setting a subnet mask is important for cloud interfaces so that the kernel
    /// generates a connected route for the subnet, enabling ARP resolution between
    /// nodes that are on the same logical network but in different namespaces.
    pub fn with_netmask(mut self, netmask: Ipv4Addr) -> Self {
        self.netmask = Some(netmask);
        self
    }

    /// Build a TAP interface with the configured parameters
    ///
    /// In test mode (with `test_helpers` feature), this creates a test shim pair.
    /// In production mode, this creates a real TAP device using tokio_tun.
    ///
    /// # Errors
    /// Returns an error if:
    /// - The TAP device cannot be created
    /// - The interface configuration fails
    pub fn build_tap(self) -> Result<Arc<Tun>> {
        #[cfg(not(feature = "test_helpers"))]
        {
            let mut builder = tokio_tun::Tun::builder().tap(true).name(&self.name).up().packet_info(false);

            if let Some(ip) = self.ip {
                builder = builder.address(ip);
            }

            if let Some(mtu) = self.mtu {
                builder = builder.mtu(mtu as i32);
            }

            if let Some(netmask) = self.netmask {
                builder = builder.netmask(netmask);
            }

            let real_tun = builder.try_build()?;

            // If a fixed MAC was requested, apply it now and re-up the interface.
            // `ip link set` briefly takes the interface down when changing the address.
            if let Some(mac) = self.mac {
                let mac_str = format!(
                    "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                );
                let status = std::process::Command::new("ip")
                    .args(["link", "set", &self.name, "address", &mac_str, "up"])
                    .status()
                    .map_err(|e| anyhow::anyhow!("Failed to set MAC on {}: {}", self.name, e))?;
                if !status.success() {
                    anyhow::bail!(
                        "ip link set {} address {} up failed with exit code {:?}",
                        self.name,
                        mac_str,
                        status.code()
                    );
                }
            }

            // Shrink the kernel transmit queue so sk_buff memory stays proportional
            // to the number of live channels rather than pre-allocated to worst case.
            let status = std::process::Command::new("ip")
                .args([
                    "link",
                    "set",
                    &self.name,
                    "txqueuelen",
                    &TAP_TXQUEUELEN.to_string(),
                ])
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to set txqueuelen on {}: {}", self.name, e))?;
            if !status.success() {
                tracing::warn!(
                    iface = %self.name,
                    txqueuelen = TAP_TXQUEUELEN,
                    "ip link set txqueuelen failed (non-fatal)"
                );
            }

            // Set promiscuous mode to ensure broadcast/multicast reception
            let status = std::process::Command::new("ip")
                .args(["link", "set", &self.name, "promisc", "on"])
                .status()
                .map_err(|e| anyhow::anyhow!("Failed to set promiscuous mode on {}: {}", self.name, e))?;
            if !status.success() {
                tracing::warn!(iface = %self.name, "ip link set promisc on failed (non-fatal)");
            }

            Ok(Arc::new(Tun::new_real(real_tun)))
        }

        #[cfg(feature = "test_helpers")]
        {
            let (tun_a, _peer) = node_lib::test_helpers::util::mk_shim_pair();
            Ok(Arc::new(tun_a))
        }
    }
}

impl Clone for InterfaceBuilder {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            ip: self.ip,
            mtu: self.mtu,
            netmask: self.netmask,
            mac: self.mac,
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "test_helpers")]
    use super::*;

    #[test]
    #[cfg(feature = "test_helpers")]
    fn creates_interface_with_name() {
        let result = InterfaceBuilder::new("test_vanet").build_tap();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(feature = "test_helpers")]
    fn creates_interface_with_ip() {
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let result = InterfaceBuilder::new("test_virtual")
            .with_ip(ip)
            .build_tap();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(feature = "test_helpers")]
    fn creates_interface_with_ip_and_mtu() {
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let result = InterfaceBuilder::new("test_configured")
            .with_ip(ip)
            .with_mtu(1400)
            .build_tap();
        assert!(result.is_ok());
    }

    #[test]
    #[cfg(feature = "test_helpers")]
    fn builder_is_reusable() {
        let base_builder = InterfaceBuilder::new("test_base");

        let iface1 = base_builder.clone().build_tap();
        assert!(iface1.is_ok());
    }
}
