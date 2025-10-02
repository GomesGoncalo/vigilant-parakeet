use crate::args::{RsuArgs, RsuParameters};
use crate::control::Rsu;
use anyhow::{anyhow, Result};
use common::device::Device;
use common::tun::Tun;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Builder for constructing Rsu instances with flexible configuration
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
    bind: String,
    tap_name: Option<String>,
    ip: Option<Ipv4Addr>,
    mtu: i32,
    hello_history: u32,
    hello_periodicity: u32,
    cached_candidates: u32,
    enable_encryption: bool,
    // For testing with injected dependencies
    #[cfg_attr(not(test), allow(dead_code))]
    tun: Option<Arc<Tun>>,
    #[cfg_attr(not(test), allow(dead_code))]
    device: Option<Arc<Device>>,
}

impl RsuBuilder {
    /// Create a new RsuBuilder with required parameters
    ///
    /// # Arguments
    /// * `bind` - Interface to bind to
    /// * `hello_periodicity` - Period in milliseconds between hello broadcasts
    pub fn new(bind: impl Into<String>, hello_periodicity: u32) -> Self {
        Self {
            bind: bind.into(),
            tap_name: None,
            ip: None,
            mtu: 1436,
            hello_history: 10,
            hello_periodicity,
            cached_candidates: 3,
            enable_encryption: false,
            tun: None,
            device: None,
        }
    }

    /// Create builder from existing RsuArgs
    pub fn from_args(args: RsuArgs) -> Self {
        Self {
            bind: args.bind,
            tap_name: args.tap_name,
            ip: args.ip,
            mtu: args.mtu,
            hello_history: args.rsu_params.hello_history,
            hello_periodicity: args.rsu_params.hello_periodicity,
            cached_candidates: args.rsu_params.cached_candidates,
            enable_encryption: args.rsu_params.enable_encryption,
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

    /// Set the hello broadcast periodicity in milliseconds
    pub fn with_hello_periodicity(mut self, period_ms: u32) -> Self {
        self.hello_periodicity = period_ms;
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

    /// Inject a test TUN device (for testing only)
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn with_tun(mut self, tun: Arc<Tun>) -> Self {
        self.tun = Some(tun);
        self
    }

    /// Inject a test Device (for testing only)
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn with_device(mut self, device: Arc<Device>) -> Self {
        self.device = Some(device);
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
        use tokio_tun::Tun as RealTokioTun;

        let args = self.to_args();

        // Create real TUN device
        let real_tun: RealTokioTun = if let Some(ip) = args.ip {
            RealTokioTun::builder()
                .name(args.tap_name.as_ref().unwrap_or(&String::default()))
                .tap()
                .mtu(args.mtu)
                .up()
                .address(ip)
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no tun devices returned from TokioTun builder"))?
        } else {
            RealTokioTun::builder()
                .name(args.tap_name.as_ref().unwrap_or(&String::default()))
                .mtu(args.mtu)
                .tap()
                .up()
                .build()?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("no tun devices returned from TokioTun builder"))?
        };

        let tun = Arc::new(Tun::new_real(real_tun));
        let device = Arc::new(Device::new(&args.bind)?);

        Rsu::new(args, tun, device)
    }

    /// Build the Rsu instance (test mode)
    ///
    /// In test mode, TUN and Device must be provided via with_tun() and with_device()
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn build(self) -> Result<Arc<Rsu>> {
        let args = self.to_args();

        let tun = self
            .tun
            .ok_or_else(|| anyhow!("TUN device required in test mode - use with_tun()"))?;
        let device = self
            .device
            .ok_or_else(|| anyhow!("Device required in test mode - use with_device()"))?;

        Rsu::new(args, tun, device)
    }

    /// Convert builder to RsuArgs
    fn to_args(&self) -> RsuArgs {
        RsuArgs {
            bind: self.bind.clone(),
            tap_name: self.tap_name.clone(),
            ip: self.ip,
            mtu: self.mtu,
            rsu_params: RsuParameters {
                hello_history: self.hello_history,
                hello_periodicity: self.hello_periodicity,
                cached_candidates: self.cached_candidates,
                enable_encryption: self.enable_encryption,
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
        assert_eq!(builder.bind, "eth0");
        assert_eq!(builder.hello_periodicity, 5000);
        assert_eq!(builder.mtu, 1436);
        assert_eq!(builder.hello_history, 10);
        assert_eq!(builder.cached_candidates, 3);
        assert!(!builder.enable_encryption);
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

        assert_eq!(builder.ip, Some("192.168.1.1".parse().unwrap()));
        assert_eq!(builder.mtu, 1500);
        assert_eq!(builder.hello_history, 20);
        assert_eq!(builder.hello_periodicity, 3000);
        assert_eq!(builder.cached_candidates, 5);
        assert!(builder.enable_encryption);
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
        assert_eq!(builder.bind, "eth1");
        assert_eq!(builder.tap_name, Some("tap0".to_string()));
        assert_eq!(builder.hello_history, 15);
        assert_eq!(builder.hello_periodicity, 4000);
        assert!(builder.enable_encryption);
    }
}
