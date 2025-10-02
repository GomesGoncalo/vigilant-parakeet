use crate::args::{ObuArgs, ObuParameters};
use crate::control::Obu;
use anyhow::{anyhow, Result};
use common::device::Device;
use common::tun::Tun;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Builder for constructing Obu instances with flexible configuration
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
    bind: String,
    tap_name: Option<String>,
    ip: Option<Ipv4Addr>,
    mtu: i32,
    hello_history: u32,
    cached_candidates: u32,
    enable_encryption: bool,
    // For testing with injected dependencies
    #[allow(dead_code)]
    tun: Option<Arc<Tun>>,
    #[allow(dead_code)]
    device: Option<Arc<Device>>,
}

impl ObuBuilder {
    /// Create a new ObuBuilder with required bind interface
    pub fn new(bind: impl Into<String>) -> Self {
        Self {
            bind: bind.into(),
            tap_name: None,
            ip: None,
            mtu: 1436,
            hello_history: 10,
            cached_candidates: 3,
            enable_encryption: false,
            tun: None,
            device: None,
        }
    }

    /// Create builder from existing ObuArgs
    pub fn from_args(args: ObuArgs) -> Self {
        Self {
            bind: args.bind,
            tap_name: args.tap_name,
            ip: args.ip,
            mtu: args.mtu,
            hello_history: args.obu_params.hello_history,
            cached_candidates: args.obu_params.cached_candidates,
            enable_encryption: args.obu_params.enable_encryption,
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

        Obu::new(args, tun, device)
    }

    /// Build the Obu instance (test mode)
    ///
    /// In test mode, TUN and Device must be provided via with_tun() and with_device()
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn build(self) -> Result<Arc<Obu>> {
        let args = self.to_args();

        let tun = self
            .tun
            .ok_or_else(|| anyhow!("TUN device required in test mode - use with_tun()"))?;
        let device = self
            .device
            .ok_or_else(|| anyhow!("Device required in test mode - use with_device()"))?;

        Obu::new(args, tun, device)
    }

    /// Convert builder to ObuArgs
    fn to_args(&self) -> ObuArgs {
        ObuArgs {
            bind: self.bind.clone(),
            tap_name: self.tap_name.clone(),
            ip: self.ip,
            mtu: self.mtu,
            obu_params: ObuParameters {
                hello_history: self.hello_history,
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
        let builder = ObuBuilder::new("eth0");
        assert_eq!(builder.bind, "eth0");
        assert_eq!(builder.mtu, 1436);
        assert_eq!(builder.hello_history, 10);
        assert_eq!(builder.cached_candidates, 3);
        assert!(!builder.enable_encryption);
    }

    #[test]
    fn builder_fluent_api() {
        let builder = ObuBuilder::new("eth0")
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
        assert_eq!(builder.bind, "eth1");
        assert_eq!(builder.tap_name, Some("tap0".to_string()));
        assert_eq!(builder.hello_history, 15);
        assert!(builder.enable_encryption);
    }
}
