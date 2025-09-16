# server_lib crate — architecture

Purpose: concrete Server node implementation. Owns a UDP server that binds to a network interface.

```mermaid
flowchart TB
  subgraph server_lib
    SV[\"control:: (Server UDP socket)\""]
    AR[\"args:: (ServerArgs, ServerParameters)\""]
  end

  SV --> CMN[\"common::Tun, common::Device\""]
```

Key responsibilities
- Parse CLI/config into `ServerArgs` and `ServerParameters`.
- Create and own `common::Tun` and `common::Device` instances.
- Bind a UDP socket to the specified network interface and port.
- Handle incoming UDP packets (currently implements a simple echo server).

Configuration
- `ServerParameters`
  - `bind_port: u16` (default: 8080)

APIs
- `create(args: ServerArgs) -> Arc<dyn Node>`: construct with a real TUN.
- `create_with_vdev(args: ServerArgs, tun: Arc<Tun>, device: Arc<Device>) -> Arc<dyn Node>`: inject shims (used by simulator/tests).

See also
- `simulator/src/node_factory.rs` for how the simulator builds Servers from YAML.