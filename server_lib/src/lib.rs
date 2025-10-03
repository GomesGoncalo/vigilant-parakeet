pub mod args;
pub use args::{ServerArgs, ServerParameters};

pub mod builder;
pub use builder::ServerBuilder;

mod server;
pub use server::Server;

use anyhow::Result;
use std::sync::Arc;

/// Create a Server instance from ServerArgs and start it
pub async fn create(args: ServerArgs) -> Result<Arc<Server>> {
    let server = ServerBuilder::from_args(args).build()?;
    server.start().await?;
    Ok(server)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn create_server_from_args() -> Result<()> {
        let args = ServerArgs {
            ip: Ipv4Addr::new(127, 0, 0, 1),
            server_params: ServerParameters { port: 0 }, // Use port 0 for OS assignment
        };
        let _server = create(args).await?;
        Ok(())
    }
}
