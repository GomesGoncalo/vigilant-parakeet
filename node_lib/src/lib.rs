pub mod args;
pub use args::Args;
pub mod control;
mod data;
pub mod messages;
pub mod metrics;
// Re-export test helpers for integration tests.
// Make this available unconditionally so integration tests can import
// `node_lib::test_helpers::hub` without passing a feature flag.
// The helper code is small and test-oriented; keeping it always exported
// avoids CI friction when running integration tests.
pub mod test_helpers {
    pub mod hub;
    pub mod util;
}

#[cfg(not(any(test, feature = "test_helpers")))]
use anyhow::Context;
use anyhow::Result;
use args::NodeType;
use common::device::Device;
use common::tun::Tun;
use std::sync::Arc;

use std::any::Any;

pub trait Node: Send + Sync {
    /// For runtime downcasting to concrete node types.
    fn as_any(&self) -> &dyn Any;
}

impl Node for control::rsu::Rsu {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Node for control::obu::Obu {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create_with_vdev(
    args: Args,
    tun: Arc<Tun>,
    node_device: Arc<Device>,
) -> Result<Arc<dyn Node>> {
    match args.node_params.node_type {
        NodeType::Rsu => Ok(control::rsu::Rsu::new(args, tun, node_device)?),
        NodeType::Obu => Ok(control::obu::Obu::new(args, tun, node_device)?),
    }
}

#[cfg(not(any(test, feature = "test_helpers")))]
pub fn create(args: Args) -> Result<Arc<dyn Node>> {
    // Use the real tokio_tun builder type in non-test builds.
    use tokio_tun::Tun as RealTokioTun;

    let real_tun: RealTokioTun = if args.ip.is_some() {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .tap()
            .mtu(args.mtu)
            .up()
            .address(args.ip.context("no ip")?)
            .build()?
            .into_iter()
            .next()
            .expect("Expecting at least 1 item in vec")
    } else {
        RealTokioTun::builder()
            .name(args.tap_name.as_ref().unwrap_or(&String::default()))
            .mtu(args.mtu)
            .tap()
            .up()
            .build()?
            .into_iter()
            .next()
            .expect("Expecting at least 1 item in vec")
    };

    // Use From/Into impl to convert the concrete real_tun into our `Tun`.
    let tun = Arc::new(Tun::from(real_tun));

    let dev = Device::new(&args.bind)?;
    create_with_vdev(args, tun, dev.into())
}

#[cfg(any(test, feature = "test_helpers"))]
pub fn create(_args: Args) -> Result<Arc<dyn Node>> {
    anyhow::bail!("create() with TokioTun::builder() is unavailable in test builds")
}

/// Initialize a tracing subscriber for tests. Safe to call multiple times.
pub fn init_test_tracing() {
    use std::sync::Once;
    static START: Once = Once::new();
    START.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::device::{Device, DeviceIo};
    use common::tun::test_tun::TokioTun;
    use std::os::unix::io::FromRawFd;
    use tokio::io::unix::AsyncFd;

    fn make_dev(mac: mac_address::MacAddress) -> Device {
        let mut fds = [0; 2];
        unsafe {
            let r = libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr());
            assert_eq!(r, 0, "socketpair failed");
            let _ = libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
        }
        Device::from_asyncfd_for_bench(
            mac,
            AsyncFd::new(unsafe { DeviceIo::from_raw_fd(fds[0]) }).unwrap(),
        )
    }

    #[tokio::test]
    async fn create_with_vdev_obu_and_rsu() {
        let (a, b) = TokioTun::new_pair();
        let tun_a = Arc::new(Tun::new_shim(a));
        let tun_b = Arc::new(Tun::new_shim(b));
        let mac_rsu: mac_address::MacAddress = [1, 2, 3, 4, 5, 6].into();
        let mac_obu: mac_address::MacAddress = [10, 11, 12, 13, 14, 15].into();
        let dev_rsu = Arc::new(make_dev(mac_rsu));
        let dev_obu = Arc::new(make_dev(mac_obu));

        let args_rsu = Args {
            bind: String::from("unused"),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: args::NodeParameters {
                node_type: NodeType::Rsu,
                hello_history: 10,
                hello_periodicity: Some(100),
                cached_candidates: 3,
            },
        };
        let args_obu = Args {
            bind: String::from("unused"),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: args::NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 10,
                hello_periodicity: None,
                cached_candidates: 3,
            },
        };

        let rsu = create_with_vdev(args_rsu, tun_a, dev_rsu).expect("rsu created");
        let obu = create_with_vdev(args_obu, tun_b, dev_obu).expect("obu created");

        // Downcast via Node::as_any to ensure trait object works
        assert!(rsu.as_any().downcast_ref::<control::rsu::Rsu>().is_some());
        assert!(obu.as_any().downcast_ref::<control::obu::Obu>().is_some());
    }

    #[test]
    fn create_in_test_build_bails() {
        // In test builds, create() should bail with an error; exercise that path.
        let args = Args {
            bind: String::from("unused"),
            tap_name: None,
            ip: None,
            mtu: 1500,
            node_params: args::NodeParameters {
                node_type: NodeType::Obu,
                hello_history: 1,
                hello_periodicity: None,
                cached_candidates: 3,
            },
        };
        let res = crate::create(args);
        assert!(res.is_err());
        if let Err(err) = res {
            assert!(err.to_string().contains("unavailable"));
        }
    }

    #[test]
    fn init_test_tracing_is_idempotent() {
        // Safe to call repeatedly without panic
        super::init_test_tracing();
        super::init_test_tracing();
    }
}
