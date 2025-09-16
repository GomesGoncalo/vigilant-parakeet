use std::net::Ipv4Addr;

#[derive(Clone, Debug)]
pub struct ServerParameters {
    /// Bind port for the UDP server
    pub bind_port: u16,
}

impl Default for ServerParameters {
    fn default() -> Self {
        Self { bind_port: 8080 }
    }
}

#[derive(Clone, Debug)]
pub struct ServerArgs {
    /// Interface to bind to
    pub bind: String,

    /// Virtual device name
    pub tap_name: Option<String>,

    /// IP address
    pub ip: Option<Ipv4Addr>,

    /// MTU
    pub mtu: i32,

    /// Server-specific parameters
    pub server_params: ServerParameters,
}
