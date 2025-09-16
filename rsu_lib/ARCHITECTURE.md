# rsu_lib crate — architecture

Purpose: concrete RSU node implementation. Owns the RSU control plane and CLI args.

```mermaid
flowchart TB
  subgraph rsu_lib
    RS["control:: (Rsu state machine)"]
    AR["args:: (RsuArgs, RsuParameters)"]
  end

  RS --> NL["node_lib::control (routing_utils, route, client_cache)"]
  RS --> MS["node_lib::messages"]
  RS --> MET["node_lib::metrics (feature: stats)"]
  RS --> CMN["common::Tun, common::Device"]
```

Key responsibilities
- Parse CLI/config into `RsuArgs` and `RsuParameters`.
- Create and own `common::Tun` and `common::Device` instances.
- Emit periodic Heartbeat control messages and handle replies.
- Maintain routing advertisements for downstream nodes.

Configuration
- `RsuParameters`
  - `hello_history: u32`
  - `hello_periodicity: u32` (ms)
  - `cached_candidates: u32` (optional, for symmetry with OBU)
  - `enable_encryption: bool` (optional)

APIs
- `create(args: RsuArgs) -> Arc<dyn Node>`: construct with a real TUN.
- `create_with_vdev(args: RsuArgs, tun: Arc<Tun>, device: Arc<Device>) -> Arc<dyn Node>`: inject shims (used by simulator/tests).

See also
- `simulator/src/node_factory.rs` for how the simulator builds RSUs from YAML.
