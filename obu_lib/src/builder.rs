use crate::args::{ObuArgs, ObuParameters};
use crate::control::Obu;
use anyhow::Result;
use node_lib::builder::NodeBuilder;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Builder for constructing Obu instances with flexible configuration
///
/// This builder wraps the generic `NodeBuilder` from `node_lib` and provides
/// OBU-specific configuration and construction logic.
///
/// # Examples
///
/// ```no_run
/// use obu_lib::builder::ObuBuilder;
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// let obu = ObuBuilder::new("eth0")
///     .with_ip("192.168.1.100".parse()?)
///     .with_hello_history(20)
///     .with_encryption(true)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct ObuBuilder {
    inner: NodeBuilder,
}

impl ObuBuilder {
    /// Create a new ObuBuilder with required bind interface
    pub fn new(bind: impl Into<String>) -> Self {
        Self {
            inner: NodeBuilder::new(bind),
        }
    }

    /// Create builder from existing ObuArgs
    pub fn from_args(args: ObuArgs) -> Self {
        let mut inner = NodeBuilder::new(args.bind);
        inner.tap_name = args.tap_name;
        inner.ip = args.ip;
        inner.mtu = args.mtu;
        inner.hello_history = args.obu_params.hello_history;
        inner.cached_candidates = args.obu_params.cached_candidates;
        inner.enable_encryption = args.obu_params.enable_encryption;
        Self { inner }
    }

    /// Set the TAP device name
    pub fn with_tap_name(mut self, name: impl Into<String>) -> Self {
        self.inner = self.inner.with_tap_name(name);
        self
    }

    /// Set the IP address
    pub fn with_ip(mut self, ip: Ipv4Addr) -> Self {
        self.inner = self.inner.with_ip(ip);
        self
    }

    /// Set the MTU (default: 1436)
    pub fn with_mtu(mut self, mtu: i32) -> Self {
        self.inner = self.inner.with_mtu(mtu);
        self
    }

    /// Set the hello history size (default: 10)
    pub fn with_hello_history(mut self, history: u32) -> Self {
        self.inner = self.inner.with_hello_history(history);
        self
    }

    /// Set the number of cached upstream candidates (default: 3)
    pub fn with_cached_candidates(mut self, count: u32) -> Self {
        self.inner = self.inner.with_cached_candidates(count);
        self
    }

    /// Enable or disable encryption (default: false)
    pub fn with_encryption(mut self, enabled: bool) -> Self {
        self.inner = self.inner.with_encryption(enabled);
        self
    }

    /// Set the node name for tracing/logging identification
    pub fn with_node_name(mut self, name: impl Into<String>) -> Self {
        self.inner = self.inner.with_node_name(name);
        self
    }

    /// Inject a test TUN device (for testing only)
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn with_tun(mut self, tun: Arc<common::tun::Tun>) -> Self {
        self.inner = self.inner.with_tun(tun);
        self
    }

    /// Inject a test Device (for testing only)
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn with_device(mut self, device: Arc<common::device::Device>) -> Self {
        self.inner = self.inner.with_device(device);
        self
    }

    /// Build the Obu instance
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - TUN device creation fails (production mode)
    /// - Device creation fails (production mode)
    /// - OBU initialization fails
    #[cfg(not(any(test, feature = "test_helpers")))]
    pub fn build(self) -> Result<Arc<Obu>> {
        let args = self.to_args();
        let tun = self.inner.create_tun_device()?;
        let device = self.inner.create_device()?;
        let node_name = self.inner.node_name.unwrap_or_else(|| "unknown".to_string());
        Obu::new(args, tun, device, node_name)
    }

    /// Build the Obu instance (test mode)
    ///
    /// In test mode, TUN and Device must be provided via with_tun() and with_device()
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn build(self) -> Result<Arc<Obu>> {
        let args = self.to_args();
        let tun = self.inner.create_tun_device()?;
        let device = self.inner.create_device()?;
        let node_name = self.inner.node_name.unwrap_or_else(|| "unknown".to_string());
        Obu::new(args, tun, device, node_name)
    }

    /// Convert builder to ObuArgs
    fn to_args(&self) -> ObuArgs {
        ObuArgs {
            bind: self.inner.bind.clone(),
            tap_name: self.inner.tap_name.clone(),
            ip: self.inner.ip,
            mtu: self.inner.mtu,
            obu_params: ObuParameters {
                hello_history: self.inner.hello_history,
                cached_candidates: self.inner.cached_candidates,
                enable_encryption: self.inner.enable_encryption,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_defaults() {
        let builder = ObuBuilder::new("eth0");
        assert_eq!(builder.inner.bind, "eth0");
        assert_eq!(builder.inner.mtu, 1436);
        assert_eq!(builder.inner.hello_history, 10);
        assert_eq!(builder.inner.cached_candidates, 3);
        assert!(!builder.inner.enable_encryption);
    }

    #[test]
    fn builder_fluent_api() {
        let builder = ObuBuilder::new("eth0")
            .with_ip("192.168.1.100".parse().unwrap())
            .with_mtu(1500)
            .with_hello_history(20)
            .with_cached_candidates(5)
            .with_encryption(true);

        assert_eq!(builder.inner.ip, Some("192.168.1.100".parse().unwrap()));
        assert_eq!(builder.inner.mtu, 1500);
        assert_eq!(builder.inner.hello_history, 20);
        assert_eq!(builder.inner.cached_candidates, 5);
        assert!(builder.inner.enable_encryption);
    }

    #[test]
    fn builder_from_args() {
        let args = ObuArgs {
            bind: "eth1".to_string(),
            tap_name: Some("tap0".to_string()),
            ip: Some("10.0.0.1".parse().unwrap()),
            mtu: 1500,
            obu_params: ObuParameters {
                hello_history: 15,
                cached_candidates: 4,
                enable_encryption: true,
            },
        };

        let builder = ObuBuilder::from_args(args);
        assert_eq!(builder.inner.bind, "eth1");
        assert_eq!(builder.inner.tap_name, Some("tap0".to_string()));
        assert_eq!(builder.inner.hello_history, 15);
        assert!(builder.inner.enable_encryption);
    }
}
