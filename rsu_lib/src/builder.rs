use crate::args::{RsuArgs, RsuParameters};
use crate::control::Rsu;
use anyhow::Result;
use node_lib::builder::NodeBuilder;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Builder for constructing Rsu instances with flexible configuration
///
/// This builder wraps the generic `NodeBuilder` from `node_lib` and provides
/// RSU-specific configuration (hello_periodicity) and construction logic.
///
/// # Examples
///
/// ```no_run
/// use rsu_lib::builder::RsuBuilder;
/// use std::sync::Arc;
///
/// # async fn example() -> anyhow::Result<()> {
/// let rsu = RsuBuilder::new("eth0", 5000)
///     .with_ip("192.168.1.1".parse()?)
///     .with_hello_history(20)
///     .with_encryption(true)
///     .build()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct RsuBuilder {
    inner: NodeBuilder,
    hello_periodicity: u32,
}

impl RsuBuilder {
    /// Create a new RsuBuilder with required parameters
    ///
    /// # Arguments
    /// * `bind` - Interface to bind to
    /// * `hello_periodicity` - Period in milliseconds between hello broadcasts
    pub fn new(bind: impl Into<String>, hello_periodicity: u32) -> Self {
        Self {
            inner: NodeBuilder::new(bind),
            hello_periodicity,
        }
    }

    /// Create builder from existing RsuArgs
    pub fn from_args(args: RsuArgs) -> Self {
        let mut inner = NodeBuilder::new(args.bind);
        inner.tap_name = args.tap_name;
        inner.ip = args.ip;
        inner.mtu = args.mtu;
        inner.hello_history = args.rsu_params.hello_history;
        inner.cached_candidates = args.rsu_params.cached_candidates;
        inner.enable_encryption = args.rsu_params.enable_encryption;
        Self {
            inner,
            hello_periodicity: args.rsu_params.hello_periodicity,
        }
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

    /// Set the hello broadcast periodicity in milliseconds
    pub fn with_hello_periodicity(mut self, period_ms: u32) -> Self {
        self.hello_periodicity = period_ms;
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

    /// Build the Rsu instance
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - TUN device creation fails (production mode)
    /// - Device creation fails (production mode)
    /// - RSU initialization fails
    #[cfg(not(any(test, feature = "test_helpers")))]
    pub fn build(self) -> Result<Arc<Rsu>> {
        let args = self.to_args();
        let tun = self.inner.create_tun_device()?;
        let device = self.inner.create_device()?;
        Rsu::new(args, tun, device)
    }

    /// Build the Rsu instance (test mode)
    ///
    /// In test mode, TUN and Device must be provided via with_tun() and with_device()
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn build(self) -> Result<Arc<Rsu>> {
        let args = self.to_args();
        let tun = self.inner.get_tun_device()?;
        let device = self.inner.get_device()?;
        Rsu::new(args, tun, device)
    }

    /// Convert builder to RsuArgs
    fn to_args(&self) -> RsuArgs {
        RsuArgs {
            bind: self.inner.bind.clone(),
            tap_name: self.inner.tap_name.clone(),
            ip: self.inner.ip,
            mtu: self.inner.mtu,
            rsu_params: RsuParameters {
                hello_history: self.inner.hello_history,
                hello_periodicity: self.hello_periodicity,
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
        let builder = RsuBuilder::new("eth0", 5000);
        assert_eq!(builder.inner.bind, "eth0");
        assert_eq!(builder.hello_periodicity, 5000);
        assert_eq!(builder.inner.mtu, 1436);
        assert_eq!(builder.inner.hello_history, 10);
        assert_eq!(builder.inner.cached_candidates, 3);
        assert!(!builder.inner.enable_encryption);
    }

    #[test]
    fn builder_fluent_api() {
        let builder = RsuBuilder::new("eth0", 5000)
            .with_ip("192.168.1.1".parse().unwrap())
            .with_mtu(1500)
            .with_hello_history(20)
            .with_hello_periodicity(3000)
            .with_cached_candidates(5)
            .with_encryption(true);

        assert_eq!(builder.inner.ip, Some("192.168.1.1".parse().unwrap()));
        assert_eq!(builder.inner.mtu, 1500);
        assert_eq!(builder.inner.hello_history, 20);
        assert_eq!(builder.hello_periodicity, 3000);
        assert_eq!(builder.inner.cached_candidates, 5);
        assert!(builder.inner.enable_encryption);
    }

    #[test]
    fn builder_from_args() {
        let args = RsuArgs {
            bind: "eth1".to_string(),
            tap_name: Some("tap0".to_string()),
            ip: Some("10.0.0.1".parse().unwrap()),
            mtu: 1500,
            rsu_params: RsuParameters {
                hello_history: 15,
                hello_periodicity: 4000,
                cached_candidates: 4,
                enable_encryption: true,
            },
        };

        let builder = RsuBuilder::from_args(args);
        assert_eq!(builder.inner.bind, "eth1");
        assert_eq!(builder.inner.tap_name, Some("tap0".to_string()));
        assert_eq!(builder.inner.hello_history, 15);
        assert_eq!(builder.hello_periodicity, 4000);
        assert!(builder.inner.enable_encryption);
    }
}
