use mac_address::MacAddress;
use thiserror::Error;

/// Domain-specific errors for node operations
#[derive(Error, Debug)]
pub enum NodeError {
    #[error("Invalid message format: {0}")]
    InvalidMessage(String),

    #[error("Message parsing failed: {0}")]
    ParseError(String),

    #[error("Invalid protocol marker, expected 0x3030")]
    InvalidProtocol,

    #[error("Buffer too short: expected at least {expected} bytes, got {actual}")]
    BufferTooShort { expected: usize, actual: usize },

    #[error("Invalid MAC address in message")]
    InvalidMacAddress,

    #[error("Encryption failed: {0}")]
    EncryptionError(String),

    #[error("Decryption failed: {0}")]
    DecryptionError(String),

    #[error("Encrypted data too short: expected at least 12 bytes for nonce, got {0}")]
    EncryptedDataTooShort(usize),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Network send failed: {0}")]
    SendError(String),

    #[error("Network receive failed: {0}")]
    RecvError(String),

    #[error("Device error: {0}")]
    DeviceError(String),

    #[error("TUN/TAP interface error: {0}")]
    TunError(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Domain-specific errors for routing operations
#[derive(Error, Debug)]
pub enum RoutingError {
    #[error("No route to destination {0}")]
    NoRoute(MacAddress),

    #[error("No upstream route available")]
    NoUpstreamRoute,

    #[error("Stale heartbeat with ID {0}")]
    StaleHeartbeat(u32),

    #[error("Invalid heartbeat reply")]
    InvalidHeartbeatReply,

    #[error("Heartbeat history must be at least 1, got {0}")]
    InvalidHistorySize(u32),

    #[error("Route computation failed: {0}")]
    ComputationError(String),

    #[error("Routing loop detected for {0}")]
    LoopDetected(MacAddress),

    #[error("Failed to acquire routing table lock: {0}")]
    LockError(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Domain-specific errors for control plane operations
#[derive(Error, Debug)]
pub enum ControlError {
    #[error("Invalid control message type")]
    InvalidMessageType,

    #[error("Heartbeat generation failed: {0}")]
    HeartbeatError(String),

    #[error("Failed to process control message: {0}")]
    ProcessingError(String),

    #[error("Routing error: {0}")]
    Routing(#[from] RoutingError),

    #[error("Node error: {0}")]
    Node(#[from] NodeError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Domain-specific errors for data plane operations
#[derive(Error, Debug)]
pub enum DataError {
    #[error("Invalid data packet type")]
    InvalidPacketType,

    #[error("Upstream forwarding failed: {0}")]
    UpstreamError(String),

    #[error("Downstream forwarding failed: {0}")]
    DownstreamError(String),

    #[error("Payload too large: {size} bytes exceeds maximum {max}")]
    PayloadTooLarge { size: usize, max: usize },

    #[error("Failed to forward packet: {0}")]
    ForwardError(String),

    #[error("Node error: {0}")]
    Node(#[from] NodeError),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Convenience type alias for Results using NodeError
pub type NodeResult<T> = Result<T, NodeError>;

/// Convenience type alias for Results using RoutingError
pub type RoutingResult<T> = Result<T, RoutingError>;

/// Convenience type alias for Results using ControlError
pub type ControlResult<T> = Result<T, ControlError>;

/// Convenience type alias for Results using DataError
pub type DataResult<T> = Result<T, DataError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_error_display() {
        let err = NodeError::InvalidProtocol;
        assert_eq!(err.to_string(), "Invalid protocol marker, expected 0x3030");
    }

    #[test]
    fn routing_error_no_route() {
        let mac: MacAddress = [1, 2, 3, 4, 5, 6].into();
        let err = RoutingError::NoRoute(mac);
        let msg = err.to_string();
        assert!(msg.contains("No route to destination"));
        assert!(msg.contains("01:02:03:04:05:06"));
    }

    #[test]
    fn buffer_too_short_error() {
        let err = NodeError::BufferTooShort {
            expected: 100,
            actual: 50,
        };
        let msg = err.to_string();
        assert!(msg.contains("expected at least 100 bytes"));
        assert!(msg.contains("got 50"));
    }

    #[test]
    fn error_conversion_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let node_err: NodeError = io_err.into();
        assert!(matches!(node_err, NodeError::Io(_)));
    }

    #[test]
    fn routing_error_conversion() {
        let routing_err = RoutingError::NoUpstreamRoute;
        let control_err: ControlError = routing_err.into();
        assert!(matches!(control_err, ControlError::Routing(_)));
    }

    #[test]
    fn encryption_error_display() {
        let err = NodeError::EncryptionError("key derivation failed".to_string());
        assert_eq!(err.to_string(), "Encryption failed: key derivation failed");
    }

    #[test]
    fn encrypted_data_too_short() {
        let err = NodeError::EncryptedDataTooShort(8);
        let msg = err.to_string();
        assert!(msg.contains("at least 12 bytes"));
        assert!(msg.contains("got 8"));
    }

    #[test]
    fn stale_heartbeat_error() {
        let err = RoutingError::StaleHeartbeat(42);
        assert_eq!(err.to_string(), "Stale heartbeat with ID 42");
    }

    #[test]
    fn payload_too_large() {
        let err = DataError::PayloadTooLarge {
            size: 2000,
            max: 1500,
        };
        let msg = err.to_string();
        assert!(msg.contains("2000 bytes"));
        assert!(msg.contains("maximum 1500"));
    }
}
