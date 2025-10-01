use thiserror::Error;

/// Domain-specific errors for device operations
#[derive(Error, Debug)]
pub enum DeviceError {
    #[error("Failed to create packet socket: {0}")]
    SocketCreation(String),

    #[error("Failed to bind device to interface {interface}: {source}")]
    BindError {
        interface: String,
        source: std::io::Error,
    },

    #[error("Failed to get MAC address for interface {0}")]
    MacAddressError(String),

    #[error("Failed to set device non-blocking: {0}")]
    NonBlockingError(String),

    #[error("Device send failed: {0}")]
    SendError(String),

    #[error("Device receive failed: {0}")]
    RecvError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Domain-specific errors for TUN/TAP operations
#[derive(Error, Debug)]
pub enum TunError {
    #[error("Failed to create TUN device: {0}")]
    CreationError(String),

    #[error("Failed to configure TUN device: {0}")]
    ConfigurationError(String),

    #[error("TUN send failed: {0}")]
    SendError(String),

    #[error("TUN receive failed: {0}")]
    RecvError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Domain-specific errors for channel operations (simulator)
#[derive(Error, Debug)]
pub enum ChannelError {
    #[error("Channel parameter parsing failed: {0}")]
    ParseError(String),

    #[error("Invalid latency value: {0}")]
    InvalidLatency(String),

    #[error("Invalid loss rate: {rate} (must be between 0.0 and 1.0)")]
    InvalidLossRate { rate: f64 },

    #[error("Channel send failed: {0}")]
    SendError(String),

    #[error("Packet dropped due to loss simulation")]
    PacketDropped,

    #[error("Destination MAC mismatch: expected {expected}, got {actual}")]
    MacMismatch { expected: String, actual: String },

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience type alias for Results using DeviceError
pub type DeviceResult<T> = Result<T, DeviceError>;

/// Convenience type alias for Results using TunError
pub type TunResult<T> = Result<T, TunError>;

/// Convenience type alias for Results using ChannelError
pub type ChannelResult<T> = Result<T, ChannelError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_error_display() {
        let err = DeviceError::SocketCreation("permission denied".to_string());
        assert_eq!(
            err.to_string(),
            "Failed to create packet socket: permission denied"
        );
    }

    #[test]
    fn bind_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such device");
        let err = DeviceError::BindError {
            interface: "eth0".to_string(),
            source: io_err,
        };
        let msg = err.to_string();
        assert!(msg.contains("Failed to bind device to interface eth0"));
        assert!(msg.contains("no such device"));
    }

    #[test]
    fn tun_error_display() {
        let err = TunError::CreationError("insufficient privileges".to_string());
        assert_eq!(
            err.to_string(),
            "Failed to create TUN device: insufficient privileges"
        );
    }

    #[test]
    fn channel_invalid_loss_rate() {
        let err = ChannelError::InvalidLossRate { rate: 1.5 };
        let msg = err.to_string();
        assert!(msg.contains("Invalid loss rate: 1.5"));
        assert!(msg.contains("must be between 0.0 and 1.0"));
    }

    #[test]
    fn mac_mismatch_error() {
        let err = ChannelError::MacMismatch {
            expected: "01:02:03:04:05:06".to_string(),
            actual: "ff:ff:ff:ff:ff:ff".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("expected 01:02:03:04:05:06"));
        assert!(msg.contains("got ff:ff:ff:ff:ff:ff"));
    }

    #[test]
    fn error_conversion_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let device_err: DeviceError = io_err.into();
        assert!(matches!(device_err, DeviceError::Io(_)));
    }

    #[test]
    fn packet_dropped_error() {
        let err = ChannelError::PacketDropped;
        assert_eq!(err.to_string(), "Packet dropped due to loss simulation");
    }
}
