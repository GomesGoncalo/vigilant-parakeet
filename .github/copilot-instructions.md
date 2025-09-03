# Copilot instructions for vigilant-parakeet

Purpose: Help AI coding agents be productive quickly in this Rust workspace by capturing the project’s architecture, workflows, and conventions specific to this repo.

## Architecture at a glance
- Workspace crates:
  - `common/`: Tun and Device abstractions used by all crates. Key: `common/src/tun.rs` exposes a test shim (`test_tun::TokioTun`) plus a wrapper `Tun` with `new_shim`/`new_real` and async `send_all/recv`.
  - `node_lib/`: Core node logic (control plane, data plane, wire formats). Control plane lives in `node_lib/src/control/{obu,rsu}/` and manages routing from Heartbeat/HeartbeatReply. Data plane in `node_lib/src/data/`. Wire messages in `node_lib/src/messages/`.
  - `node/`: Thin binary wiring `node_lib` and `common` to run a single node.
  - `simulator/`: Orchestrates multi-node simulations (netns, topology, HTTP API). See `simulator/src/` and `simulator/ARCHITECTURE.md`.
  - `visualization/`: Browser UI consuming simulator state.
- Data flow:
  - RSU emits periodic Heartbeat control messages.
  - OBUs and RSUs forward and reply, populating routing state; `Routing::get_route_to(Some(mac))` selects routes (prefers lower observed latency; ties by hop count). `select_and_cache_upstream(mac)` stores the chosen next hop.
  - Upstream data from an OBU uses the cached upstream route; downstream is forwarded per-route.

## Key files and patterns
- `common/src/tun.rs`: Unified `Tun` wrapper with a built-in test shim. In tests, create pairs via `TokioTun::new_pair()` and wrap with `Tun::new_shim`. Methods are async: `send_all`, `send_vectored`, `recv`.
- `common/src/device.rs`: `Device::from_asyncfd_for_bench(mac, AsyncFd<DeviceIo>)` creates a device around a raw fd (e.g., a socketpair end) used in integration tests.
- `node_lib/src/control/obu/routing.rs`:
  - Pure selection: `get_route_to(Some(target_mac)) -> Option<Route>` computes best route without mutating state.
  - Cache API: `select_and_cache_upstream(mac) -> Option<Route>` updates `cached_upstream`. `get_route_to(None)` returns the cached route.
  - Heartbeat handlers: `handle_heartbeat` inserts/forwards and may trigger cache selection; `handle_heartbeat_reply` updates downstream observations and may forward unless it would bounce.
- Logging: Uses `tracing`. Tests initialize with `node_lib::init_test_tracing()` (idempotent) to see logs under `RUST_LOG`.

## Tests and dev workflows
- Run all workspace tests:
  - `cargo test --workspace`
- Focused crate tests:
  - `cargo test -p node_lib --lib --tests`
- Code coverage (used here):
  - `cargo tarpaulin -p common -p node_lib --out Lcov --features test_helpers` (produces `lcov.info`).
- Integration tests exercising real tasks under Tokio:
  - `node_lib/tests/integration_topology.rs`: RSU+OBU over a socketpair; OBU discovers RSU as upstream. Uses Tun shim and `Device::from_asyncfd_for_bench`.
  - `node_lib/tests/integration_two_hop.rs`: RSU + 2 OBUs via a programmable hub injecting per-link delays. Verifies OBU2 prefers two-hop via OBU1 when direct link is higher latency; injects upstream via OBU2’s TUN peer.

## Conventions and gotchas
- Prefer the Tun shim over OS TUN in tests. Construct with `TokioTun::new_pair()` and wrap with `Tun::new_shim`. Avoid test-only feature gates in downstream tests; the shim is exposed in `common::tun::test_tun`.
- Routing API contract:
  - Do not mutate caches inside `get_route_to(Some(_))`; use `select_and_cache_upstream` for writes.
  - When handling HeartbeatReply, avoid forwarding back to `pkt.from` if it equals the recorded next hop (bounce prevention).
- Latency-aware selection: `get_route_to(Some(mac))` prefers observed lower latency across hops (score by min+avg). Falls back to fewest hops if no latency is known. Keep tie-breaking deterministic in changes.
- Async I/O building blocks: `tokio::io::unix::AsyncFd` over a nonblocking socketpair stands in for L2 devices in tests. Mark FDs `O_NONBLOCK`.
- Tracing over prints: Use `tracing` macros; tests can view logs via `init_test_tracing()` and `RUST_LOG` env.

## Extending or debugging
- Add routes tests in `node_lib/src/control/obu/routing.rs` alongside existing ones; mirror message sequences in unit tests using the `messages` module to build frames.
- For new end-to-end tests, prefer the device/socketpair + Tun shim pattern seen in existing integration tests. If you need to inject per-link latency, copy the simple `Hub` used in `integration_two_hop.rs`.
- When changing routing heuristics, update or add tests that assert the selection and caching behavior. Use `node_lib::init_test_tracing()` to get detailed logs during runs.

## Examples from the repo
- Creating devices for tests:
  - `let dev = Device::from_asyncfd_for_bench(mac, AsyncFd::new(DeviceIo::from_raw_fd(fd))?)` (ensure `O_NONBLOCK`).
- Tun shim pair:
  - `let (a, b) = common::tun::test_tun::TokioTun::new_pair(); let tun = Tun::new_shim(a);`
- Selecting and caching a route:
  - `if let Some(route) = routing.select_and_cache_upstream(target_mac) { /* use route.next_hop */ }`

If anything here is unclear or you discover a pattern worth documenting (e.g., additional features or simulators’ HTTP endpoints), please comment so we can refine these instructions.
