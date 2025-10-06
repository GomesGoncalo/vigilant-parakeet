use crate::args::ServerArgs;
use crate::server::Server;
use anyhow::Result;
use std::net::Ipv4Addr;
use std::sync::Arc;

/// Builder for creating Server instances
pub struct ServerBuilder {
    ip: Ipv4Addr,
    port: u16,
}

impl ServerBuilder {
    /// Create a new ServerBuilder with the specified IP address
    pub fn new(ip: Ipv4Addr) -> Self {
        Self { ip, port: 8080 }
    }

    /// Create a ServerBuilder from ServerArgs
    pub fn from_args(args: ServerArgs) -> Self {
        Self {
            ip: args.ip,
            port: args.server_params.port,
        }
    }

    /// Set the port for the server
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    /// Build the Server instance
    pub fn build(self) -> Result<Arc<Server>> {
        let server = Arc::new(Server::new(self.ip, self.port));
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
            server_params: ServerParameters { port: 7777 },
        };
        let server = ServerBuilder::from_args(args).build().unwrap();
        assert_eq!(server.ip(), Ipv4Addr::new(192, 168, 1, 1));
        assert_eq!(server.port(), 7777);
    }
}
