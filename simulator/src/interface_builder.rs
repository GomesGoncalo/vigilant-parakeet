//! Interface builder for creating network interfaces
//!
//! This module provides a unified interface for creating TAP/TUN devices
//! with consistent handling of both real and test implementations.

use anyhow::Result;
use common::tun::Tun;
use std::net::Ipv4Addr;
use std::sync::Arc;

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
///     .with_mtu(1436)
///     .build_tap()?;
/// # Ok(())
/// # }
/// ```
pub struct InterfaceBuilder {
    name: String,
    ip: Option<Ipv4Addr>,
    mtu: Option<u16>,
}

impl InterfaceBuilder {
    /// Create a new interface builder with the specified name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ip: None,
            mtu: None,
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
            let mut builder = tokio_tun::Tun::builder().tap().name(&self.name).up();

            if let Some(ip) = self.ip {
                builder = builder.address(ip);
            }

            if let Some(mtu) = self.mtu {
                builder = builder.mtu(mtu as i32);
            }

            let real_tun = builder.build()?.into_iter().next().ok_or_else(|| {
                anyhow::anyhow!(
                    "No TAP device returned from builder for interface '{}'",
                    self.name
                )
            })?;

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
            .with_mtu(1436)
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
