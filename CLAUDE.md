# CLAUDE.md — AI Assistant Guide for vigilant-parakeet

**vigilant-parakeet** is a Rust workspace that simulates and visualizes vehicular network nodes (OBU/RSU — On-Board Units / Road-Side Units) and their routing protocols, using Linux network namespaces for realistic simulation.

---

## Critical Timing — Never Cancel Long Commands

| Command | Expected Duration |
|---|---|
| `cargo build --workspace` | 90–120 seconds |
| `cargo build --workspace --release` | ~90 seconds |
| `cargo test --workspace` | ~22 seconds |
| `cargo clippy --workspace --all-targets -- -D warnings` | ~43 seconds |
| Coverage via tarpaulin | 80+ seconds |

**NEVER cancel builds, tests, clippy, or coverage runs.** Set tool timeouts to at least 180 seconds for builds and 90 seconds for tests/clippy.

---

## Workspace Structure

```
vigilant-parakeet/
├── common/          # Shared abstractions: TUN device, network interface, stats, error types
├── node_lib/        # Core node logic: routing, crypto, control plane, wire protocol, test helpers
├── obu_lib/         # OBU-specific control plane and session logic
├── rsu_lib/         # RSU-specific control plane and routing
├── server_lib/      # Server-side functionality
├── node/            # Binary crate: CLI entry point (`node obu` / `node rsu`)
├── simulator/       # Binary: multi-node orchestration with network namespaces, HTTP API, TUI
├── visualization/   # Yew/WASM browser-based dashboard (fetches metrics from simulator HTTP API)
├── scripts_tools/   # Script utility library
├── scripts/         # Development helper shell scripts
├── .cargo/          # config.toml: enables tokio_unstable rustflags
├── .github/         # CI workflows and copilot-instructions.md
└── .githooks/       # commit-msg hook (Conventional Commits enforcement)
```

**Key relationships:**
- RSU emits periodic Heartbeat control messages; OBUs/RSUs forward and reply, building routing tables
- Routing prefers lower observed latency, falls back on hop count
- Integration tests use in-memory Hub and TUN shims — no network namespaces or root required

---

## Development Workflow

### Before Every Commit

Run all checks in this order:

```bash
# 1. Format
cargo fmt

# 2. Lint (warnings are errors)
cargo clippy --workspace --all-targets -- -D warnings

# 3. Build
cargo build --workspace

# 4. Test (with test helpers feature — required for integration tests)
cargo test --workspace --features test_helpers
```

Or use the CI check script:
```bash
./scripts/ci-check.sh
```

### Quick Iteration per Crate

```bash
cargo test -p node_lib                          # Fastest iteration
cargo test -p node_lib --features stats         # Includes metric tests
RUST_LOG=trace cargo test -p node_lib -- <test_name> -- --nocapture  # With logs
```

### Feature Flags

| Flag | Purpose |
|---|---|
| `test_helpers` | Enables TUN shims and in-process Hub for non-privileged testing — **required for CI tests** |
| `stats` | Enables metrics/counters collection |
| `webview` | Enables HTTP API on port 3030 (warp-based, JSON metrics) |
| `tui` | Enables terminal UI dashboard |

---

## Testing

### Test Locations

- **Unit tests**: `*/src/tests/` directories in each crate
- **Integration tests**: `node_lib/tests/` (11 files covering topology, encryption, routing, failover, latency)

### Key Test Helpers (in `node_lib::test_helpers`)

- `hub::Hub` — In-process programmable hub simulating network with configurable latency and packet loss
- `common::tun::test_tun::TokioTun` — TUN device shim for non-privileged test environments
- `util::mk_shim_pair()` — Creates paired virtual TUN interfaces for tests

### Integration Test Commands

```bash
cargo test --workspace --features test_helpers          # Full suite (CI standard)
cargo test -p node_lib --features test_helpers          # node_lib integration tests only
cargo test -p node_lib --features "test_helpers,stats"  # With metrics
```

### Coverage

```bash
# Install tarpaulin (first time only, ~3 minutes)
cargo install cargo-tarpaulin --locked

# Run coverage (80+ seconds — NEVER CANCEL)
cargo tarpaulin -p common -p node_lib -p obu_lib -p rsu_lib \
  --out Lcov --features test_helpers --timeout 120
```

Coverage is generated on CI for `common`, `node_lib`, `obu_lib`, `rsu_lib`. The `simulator` crate is excluded because it requires root/network namespaces.

---

## Commit Conventions

**Required format (Conventional Commits):**

```
<type>(<scope>): <short description>

- What: <what changed>
- Why: <reason for change>
- How: <implementation approach>
- Testing: <validation performed>
```

**Valid types:** `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `chore`, `style`, `ci`, `build`, `revert`

**Example:**

```
feat(node_lib): add heartbeat retry mechanism for RSU nodes

- What: Implement exponential backoff retry for failed heartbeat transmissions
- Why: Improve reliability in lossy network conditions
- How: Add RetryConfig struct with configurable max_attempts and backoff_factor
- Testing: cargo test --workspace --features test_helpers (passed ~22s),
           cargo clippy --workspace --all-targets -- -D warnings (passed ~43s)
```

**Small commits with associated tests.** Each commit should be self-contained and include the tests that validate its change.

Install the commit-msg hook locally:
```bash
./scripts/install-hooks.sh
```

---

## CI Pipeline

Defined in `.github/workflows/ci.yml`:

1. **ShellCheck** — All `.sh` files linted with reviewdog
2. **Format check** — `cargo fmt --all -- --check` (5 min timeout)
3. **Clippy** — `cargo clippy --workspace --all-targets -- -D warnings` (10 min timeout)
4. **Build** — `cargo build --workspace --locked` (15 min timeout)
5. **Tests** — `cargo test --workspace --features test_helpers --locked` (10 min timeout)
6. **Coverage** (main branch) — tarpaulin on library crates, deploys badge to GitHub Pages

**All CI checks must pass before merging.**

---

## Running the Simulator

The simulator requires **Linux** and **sudo** (creates network namespaces):

```bash
# Build with HTTP API
cargo build -p simulator --release --features webview

# Run (requires sudo)
sudo RUST_LOG="node=debug" ./target/release/simulator \
  --config-file simulator.yaml --pretty
```

**Note:** `*.yaml` files are in `.gitignore` — create config files locally, don't commit them.

### HTTP API (port 3030)

```bash
# Get metrics
curl http://localhost:3030/metrics

# Set channel parameters between nodes
curl -X POST -H "Content-Type: application/json" \
  -d '{"latency":"100","loss":"0.0"}' \
  http://localhost:3030/channel/n1/n2/
```

### Example Configuration

```yaml
# simulator.yaml
nodes:
  n1:
    config_path: n1.yaml
  n2:
    config_path: n2.yaml
topology:
  n1:
    n2:
      latency: 0
      loss: 0
  n2:
    n1:
      latency: 0
      loss: 0
```

```yaml
# n1.yaml (RSU)
node_type: Rsu
hello_history: 10
hello_periodicity: 5000
ip: 10.0.0.1
```

```yaml
# n2.yaml (OBU)
node_type: Obu
hello_history: 10
ip: 10.0.0.2
```

---

## Visualization (WASM)

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk

cd visualization
trunk build --release   # Build WASM frontend
trunk serve             # Dev server
```

---

## Cargo Configuration

**`.cargo/config.toml`** sets `rustflags = ["--cfg", "tokio_unstable"]` to enable unstable Tokio features (required by the project).

**Release profile** (`Cargo.toml`):
- `codegen-units = 1`
- `lto = "fat"`
- `panic = "abort"`

**`Cargo.lock` is not tracked** (in `.gitignore`). Always build with `--locked` in CI to ensure reproducibility.

---

## Helper Scripts

All scripts are in `scripts/`:

| Script | Purpose |
|---|---|
| `ci-check.sh` | Full CI checks: fmt → clippy → build → test |
| `lint-and-fmt.sh` | Format and/or clippy with options |
| `test-all.sh` | Run tests for workspace or specific crates |
| `build-all.sh` | Build workspace or specific packages |
| `coverage.sh` | Wrapper around cargo-tarpaulin |
| `run-sim.sh` | Build and run simulator with default config |
| `install-hooks.sh` | Configure git commit-msg hook |
| `check-deps.sh` | Check tool dependencies |

---

## Key Conventions

1. **Always run `cargo fmt` before committing** — CI rejects unformatted code
2. **Always run `cargo clippy --workspace --all-targets -- -D warnings`** — warnings are errors in CI
3. **Always run tests with `--features test_helpers`** — this matches CI and enables integration tests
4. **Commits must follow Conventional Commits** — enforced by git hook and CI workflow
5. **Each commit should be small and include associated tests**
6. **Never commit `*.yaml` or `Cargo.lock`** — both are in `.gitignore`
7. **Never cancel long-running commands** — builds/tests have known timing; let them complete
8. **Simulator requires `sudo`** — network namespace creation is a privileged operation

---

## Architecture Reference

For detailed architecture, see:
- `ARCHITECTURE.md` — top-level crate relationships and data flow
- `common/ARCHITECTURE.md`, `node_lib/ARCHITECTURE.md`, `simulator/ARCHITECTURE.md`, `visualization/ARCHITECTURE.md`
- `DEVELOPMENT.md` — development notes and tips
- `CONTRIBUTING.md` — contributor guide
- `.github/copilot-instructions.md` — additional AI assistant context with timing details
