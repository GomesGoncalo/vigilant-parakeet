use crate::args::ServerArgs;
use crate::server::{Server, SharedTun};
use anyhow::Result;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Builder for creating Server instances
pub struct ServerBuilder {
    ip: Ipv4Addr,
    port: u16,
    node_name: Option<String>,
    tun: Option<SharedTun>,
    enable_encryption: bool,
}

impl ServerBuilder {
    /// Create a new ServerBuilder with the specified IP address
    pub fn new(ip: Ipv4Addr) -> Self {
        Self {
            ip,
            port: 8080,
            node_name: None,
            tun: None,
            enable_encryption: false,
        }
    }

    /// Create a ServerBuilder from ServerArgs
    pub fn from_args(args: ServerArgs) -> Self {
        Self {
            ip: args.ip,
            port: args.server_params.port,
            node_name: None,
            tun: None,
            enable_encryption: args.server_params.enable_encryption,
        }
    }

    /// Set the port for the server
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Set the node name for tracing/logging identification
    pub fn with_node_name(mut self, name: impl Into<String>) -> Self {
        self.node_name = Some(name.into());
        self
    }

    /// Set the TAP device for decapsulated OBU traffic
    pub fn with_tun(mut self, tun: SharedTun) -> Self {
        self.tun = Some(tun);
        self
    }

    /// Enable or disable encryption
    pub fn with_encryption(mut self, enable: bool) -> Self {
        self.enable_encryption = enable;
        self
    }

    /// Build the Server instance
    pub fn build(self) -> Result<Arc<Server>> {
        let node_name = self.node_name.unwrap_or_else(|| "unknown".to_string());
        let mut server = Server::new(self.ip, self.port, node_name);
        if let Some(tun) = self.tun {
            server = server.with_tun(tun);
        }
        server = server.with_encryption(self.enable_encryption);
        let server = Arc::new(server);
        Ok(server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ServerParameters;
    use std::net::Ipv4Addr;

    #[test]
    fn builder_defaults() {
        let server = ServerBuilder::new(Ipv4Addr::new(127, 0, 0, 1))
            .build()
            .unwrap();
        assert_eq!(server.ip(), Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(server.port(), 8080);
    }

    #[test]
    fn builder_with_port() {
        let server = ServerBuilder::new(Ipv4Addr::new(127, 0, 0, 1))
            .with_port(9999)
            .build()
            .unwrap();
        assert_eq!(server.port(), 9999);
    }

    #[test]
    fn builder_from_args() {
        let args = ServerArgs {
            ip: Ipv4Addr::new(192, 168, 1, 1),
            server_params: ServerParameters {
                port: 7777,
                enable_encryption: true,
            },
        };
        let server = ServerBuilder::from_args(args).build().unwrap();
        assert_eq!(server.ip(), Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(server.port(), 7777);
    }
}
