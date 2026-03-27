use crate::args::{RsuArgs, RsuParameters};
use crate::control::Rsu;
use anyhow::Result;
use node_lib::builder::NodeBuilder;
use std::sync::Arc;

/// Builder for constructing Rsu instances with flexible configuration
///
/// RSU nodes no longer have a TAP device. They only need a VANET device
/// for wireless communication and a cloud socket for server connectivity.
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
        inner.mtu = args.mtu;
        inner.hello_history = args.rsu_params.hello_history;
        inner.cached_candidates = args.rsu_params.cached_candidates;
        Self {
            inner,
            hello_periodicity: args.rsu_params.hello_periodicity,
        }
    }

    /// Set the MTU (default: 1400)
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

    /// Set the node name for tracing/logging identification
    pub fn with_node_name(mut self, name: impl Into<String>) -> Self {
        self.inner = self.inner.with_node_name(name);
        self
    }

    /// Inject a test Device (for testing only)
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn with_device(mut self, device: Arc<common::device::Device>) -> Self {
        self.inner = self.inner.with_device(device);
        self
    }

    /// Build the Rsu instance
    #[cfg(not(any(test, feature = "test_helpers")))]
    pub fn build(self) -> Result<Arc<Rsu>> {
        let args = self.to_args();
        let device = self.inner.create_device()?;
        let node_name = self
            .inner
            .node_name
            .unwrap_or_else(|| "unknown".to_string());
        Rsu::new(args, device, node_name)
    }

    /// Build the Rsu instance (test mode)
    #[cfg(any(test, feature = "test_helpers"))]
    pub fn build(self) -> Result<Arc<Rsu>> {
        let args = self.to_args();
        let device = self.inner.create_device()?;
        let node_name = self
            .inner
            .node_name
            .unwrap_or_else(|| "unknown".to_string());
        Rsu::new(args, device, node_name)
    }

    /// Convert builder to RsuArgs
    fn to_args(&self) -> RsuArgs {
        RsuArgs {
            bind: self.inner.bind.clone(),
            mtu: self.inner.mtu,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: self.inner.hello_history,
                hello_periodicity: self.hello_periodicity,
                cached_candidates: self.inner.cached_candidates,
                server_ip: None,
                server_port: 8080,
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
        assert_eq!(builder.inner.mtu, 1400);
        assert_eq!(builder.inner.hello_history, 10);
        assert_eq!(builder.inner.cached_candidates, 3);
    }

    #[test]
    fn builder_fluent_api() {
        let builder = RsuBuilder::new("eth0", 5000)
            .with_mtu(1500)
            .with_hello_history(20)
            .with_hello_periodicity(3000)
            .with_cached_candidates(5);

        assert_eq!(builder.inner.mtu, 1500);
        assert_eq!(builder.inner.hello_history, 20);
        assert_eq!(builder.hello_periodicity, 3000);
        assert_eq!(builder.inner.cached_candidates, 5);
    }

    #[test]
    fn builder_from_args() {
        let args = RsuArgs {
            bind: "eth1".to_string(),
            mtu: 1500,
            cloud_ip: None,
            rsu_params: RsuParameters {
                hello_history: 15,
                hello_periodicity: 4000,
                cached_candidates: 4,
                server_ip: None,
                server_port: 8080,
            },
        };

        let builder = RsuBuilder::from_args(args);
        assert_eq!(builder.inner.bind, "eth1");
        assert_eq!(builder.inner.hello_history, 15);
        assert_eq!(builder.hello_periodicity, 4000);
    }
}
