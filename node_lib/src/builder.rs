//! Generic Node Builder
//!
//! This module provides a generic builder pattern for constructing node instances (OBU/RSU)
//! with flexible configuration, eliminating duplication between ObuBuilder and RsuBuilder.

use anyhow::{anyhow, Result};
use common::device::Device;
use common::tun::Tun;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Generic builder for node configuration
///
/// This builder provides common configuration options shared between OBU and RSU nodes.
/// Type-specific builders wrap this to provide their custom `build()` implementations.
#[derive(Clone)]
pub struct NodeBuilder {
    pub bind: String,
    pub tap_name: Option<String>,
    pub ip: Option<Ipv4Addr>,
    pub mtu: i32,
    pub hello_history: u32,
    pub cached_candidates: u32,
    pub enable_encryption: bool,
    pub node_name: Option<String>,
    // For testing with injected dependencies
    #[cfg_attr(not(test), allow(dead_code))]
    pub tun: Option<Arc<Tun>>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub device: Option<Arc<Device>>,
}

impl NodeBuilder {
    /// Create a new NodeBuilder with required bind interface
    pub fn new(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            tap_name: None,
            ip: None,
            mtu: 1436,
            hello_history: 10,
            cached_candidates: 3,
            enable_encryption: false,
            node_name: None,
            tun: None,
            device: None,
        }
    }

    /// Set the TAP device name
    pub fn with_tap_name(mut self, name: impl Into<String>) -> Self {
        self.tap_name = Some(name.into());
        self
    }

    /// Set the IP address
    pub fn with_ip(mut self, ip: Ipv4Addr) -> Self {
        self.ip = Some(ip);
        self
    }

    /// Set the MTU (default: 1436)
    pub fn with_mtu(mut self, mtu: i32) -> Self {
        self.mtu = mtu;
        self
    }

    /// Set the hello history size (default: 10)
    pub fn with_hello_history(mut self, history: u32) -> Self {
        self.hello_history = history;
        self
    }

    /// Set the number of cached upstream candidates (default: 3)
    pub fn with_cached_candidates(mut self, count: u32) -> Self {
        self.cached_candidates = count;
        self
    }

    /// Enable or disable encryption (default: false)
    pub fn with_encryption(mut self, enabled: bool) -> Self {
        self.enable_encryption = enabled;
        self
    }

    /// Set the node name for tracing/logging identification
    pub fn with_node_name(mut self, name: impl Into<String>) -> Self {
        self.node_name = Some(name.into());
        self
    }

    /// Inject a test TUN device (for testing only)
    ///
    /// This method is always available but only useful during testing.
    /// In production builds, the injected devices are ignored.
    pub fn with_tun(mut self, tun: Arc<Tun>) -> Self {
        self.tun = Some(tun);
        self
    }

    /// Inject a test Device (for testing only)
    ///
    /// This method is always available but only useful during testing.
    /// In production builds, the injected devices are ignored.
    pub fn with_device(mut self, device: Arc<Device>) -> Self {
        self.device = Some(device);
        self
    }

    /// Create real TUN device in production mode
    ///
    /// # Errors
    ///
    /// Returns an error if TUN device creation fails
    #[cfg(not(any(test, feature = "test_helpers")))]
    pub fn create_tun_device(&self) -> Result<Arc<Tun>> {
        use tokio_tun::Tun as RealTokioTun;

        let real_tun: RealTokioTun = if let Some(ip) = self.ip {
            RealTokioTun::builder()
                .name(self.tap_name.as_ref().unwrap_or(&String::default()))
                .tap()
                .mtu(self.mtu)
                .up()
                .address(ip)
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no tun devices returned from TokioTun builder"))?
        } else {
            RealTokioTun::builder()
                .name(self.tap_name.as_ref().unwrap_or(&String::default()))
                .mtu(self.mtu)
                .tap()
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no tun devices returned from TokioTun builder"))?
        };

        Ok(Arc::new(Tun::new_real(real_tun)))
    }

    /// Get injected TUN device (test mode)
    ///
    /// # Errors
    ///
    /// Returns an error if TUN device was not provided via with_tun()
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn create_tun_device(&self) -> Result<Arc<Tun>> {
        self.tun
            .clone()
            .ok_or_else(|| anyhow!("TUN device required in test mode - use with_tun()"))
    }

    /// Create real Device (production mode)
    ///
    /// # Errors
    ///
    /// Returns an error if Device creation fails
    #[cfg(not(any(test, feature = "test_helpers")))]
    pub fn create_device(&self) -> Result<Arc<Device>> {
        Ok(Arc::new(Device::new(&self.bind)?))
    }

    /// Get injected Device (test mode)
    ///
    /// # Errors
    ///
    /// Returns an error if Device was not provided via with_device()
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn create_device(&self) -> Result<Arc<Device>> {
        self.device
            .clone()
            .ok_or_else(|| anyhow!("Device required in test mode - use with_device()"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults() {
        let builder = NodeBuilder::new("eth0");
        assert_eq!(builder.bind, "eth0");
        assert_eq!(builder.mtu, 1436);
        assert_eq!(builder.hello_history, 10);
        assert_eq!(builder.cached_candidates, 3);
        assert!(!builder.enable_encryption);
    }

    #[test]
    fn builder_fluent_api() {
        let builder = NodeBuilder::new("eth0")
            .with_ip("192.168.1.100".parse().unwrap())
            .with_mtu(1500)
            .with_hello_history(20)
            .with_cached_candidates(5)
            .with_encryption(true);

        assert_eq!(builder.ip, Some("192.168.1.100".parse().unwrap()));
        assert_eq!(builder.mtu, 1500);
        assert_eq!(builder.hello_history, 20);
        assert_eq!(builder.cached_candidates, 5);
        assert!(builder.enable_encryption);
    }
}
