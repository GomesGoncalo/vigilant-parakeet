# Development notes — vigilant-parakeet

Purpose

This file is a concise, actionable reference for maintaining and extending the
repository: how to build, run tests, use shared test helpers, and follow the
project's local conventions.

Quick commands

- Build the whole workspace:

```bash
cargo build --workspace
```

- Run the full test suite (workspace):

```bash
cargo test --workspace
```

- Run only the `node_lib` tests (fast iteration on routing/controls):

```bash
cargo test -p node_lib
```

- Run a specific test (example):

```bash
cargo test -p node_lib -- tests::integration_two_hop -- --nocapture
```

Formatting & linting

- Format code:

```bash
cargo fmt
```

- Lint with clippy (treat warnings as errors):

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Developer setup — install local git hooks
----------------------------------------

We provide a small `./scripts/install-hooks.sh` helper that configures your
clone to use the repository's bundled `.githooks` directory. This is optional
but recommended: the CI enforces commit message format and other checks, and
enabling local hooks gives early warnings before you push.

One-liner (run once per clone):

```bash
# enable the repo hooks for this clone
git config core.hooksPath .githooks
chmod +x .githooks/*
```

Or run the convenience script included in the repo:

```bash
./scripts/install-hooks.sh
```

Notes:
- This action is explicit and local-only (git does not allow repos to auto-run
  or force hooks on clones for security reasons).
- CI is authoritative; the GitHub Actions workflow will still fail PRs if
  commit subjects do not match the Conventional Commits format. Local hooks
  only provide warnings to help authors before pushing.


Testing notes

- Unit tests and integration tests are Rust-native. Integration tests live in
  `node_lib/tests/` and use in-process helpers (no network namespaces required
  to run them).

- Integration tests that exercise the simulator/network namespaces are kept in
  `simulator/` and may require `sudo` to create namespaces and veth pairs; the
  included `scripts/run-sim.sh` wraps a typical simulator run.

Shared test helpers

- Use the canonical `Hub` test helper rather than copying an ad-hoc hub into
  each test file. The shared helper is available at:

```rust
use node_lib::test_helpers::hub::Hub;
```

  - The repository provides `node_lib/tests/hub.rs` that re-exports the shared
    helper for convenience inside crate tests.
  - The `Hub` can simulate per-link latency/loss and provides upstream/
    downstream watch hooks to assert traffic seen by the hub.

- For TUN-level testing use the shim exposed in `common`:

```rust
let (a, b) = common::tun::test_tun::TokioTun::new_pair();
let tun = common::Tun::new_shim(a);
```

  This avoids creating OS TUN devices in CI or local development.

Running the simulator locally

- The simulator manages network namespaces. Build first then run with `sudo`:

```bash
cargo build -p simulator --release
sudo RUST_LOG=info ./target/release/simulator --config-file examples/simulator.yaml --pretty
```

- The simulator exposes an HTTP API (default port 3030) for runtime stats and
  channel control.

Config & features

- To enable runtime metrics / counters (cheap atomics) enable the `stats`
  feature when building:

```bash
cargo test -p node_lib --features stats
cargo build --features stats
```

Best practices for tests

- Prefer small, deterministic tests. When adding tests that exercise routing
  heuristics, use the provided `TokioTun` + `Hub` helpers so tests are
  reproducible and fast.

- `node_lib` exposes its `test_helpers` module from the crate root so tests
  can import helpers without extra feature flags.

- Keep the `routes` caching contract in mind when writing tests:
  - `get_route_to(Some(mac))` is pure and must not mutate cache state.
  - `select_and_cache_upstream(mac)` is the write API used to populate the
    cached upstream and the N-best candidate list.

Housekeeping

- If you see a compiler warning like "unused import: Ordering" in tests, run
  `cargo fix --test <testname>` or remove the unused import to keep CI clean.

- When you change public behavior, add tests (unit + 1-2 integration) to cover
  the change.

Troubleshooting

- If a test that uses the `Hub` flakes, run it with `--nocapture` and
  `RUST_LOG=trace` to see the hub logs and crate traces:

```bash
RUST_LOG=trace cargo test -p node_lib -- tests::integration_two_hop -- --nocapture
```

- For simulator runs that create namespaces, ensure you have `sudo` and
  that no leftover `sim_ns_*` namespaces remain from a prior run (clean up
  with `ip netns list` and `ip netns delete <name>` if needed).

Contributing

- Branch from `main` or the appropriate feature branch.
- Add tests for behavioral changes.
- Run formatting and clippy locally before opening a PR.

```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Contact

- If a change is non-trivial or affects simulation semantics, open an issue so
  we can discuss design and test coverage patterns before implementation.

---

This file is intentionally terse: put longer how-tos in `docs/` if we need
step-by-step tutorials or screenshots for the visualization UI.
