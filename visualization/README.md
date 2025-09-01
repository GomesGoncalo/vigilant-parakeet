# visualization (frontend)

Short notes and quick commands for building and testing the visualization crate used in this repository.

## Overview

This crate contains the Yew/WASM visualization components and helper code used by the simulator frontend. It exposes small pure helpers that are covered by both host unit tests and wasm-bindgen tests.

## Requirements

- Rust + cargo
- wasm32 target: `wasm32-unknown-unknown`
- `wasm-pack` (recommended for running wasm-bindgen tests)
- A headless-capable browser installed (Firefox or Chrome) for wasm tests
- `trunk` to build the frontend (optional, for producing release assets)

Install helpful tools:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack    # optional, recommended
cargo install trunk        # only if you want to build the Yew app with trunk
```

## Run host unit tests

Run the normal Rust tests (fast):

```bash
cd visualization
cargo test
```

## Compile wasm tests (produce .wasm, no run)

This will compile the wasm test artifacts without attempting to execute them:

```bash
cd visualization
cargo test --target wasm32-unknown-unknown --no-run
```

Note: attempting to execute the produced `.wasm` directly will produce `Exec format error`. Use a wasm test runner (below) to execute tests in a browser.

## Run wasm-bindgen tests (recommended: wasm-pack)

The easiest and most reliable way to run wasm-bindgen tests locally is `wasm-pack test` which runs the tests inside a browser (headless):

```bash
cd visualization
wasm-pack test --headless --firefox
```

Use `--chrome` instead of `--firefox` if you prefer Chrome.

## Build frontend (release)

To build the Yew frontend assets with `trunk`:

```bash
cd visualization
trunk build --release
```

## Troubleshooting

- "Exec format error": you tried to execute a .wasm file directly. Use `wasm-pack test` or a wasm test runner that launches a browser.
- If `wasm-pack` is not available via `cargo install`, consider installing from your platform package manager or download from the project releases.
- Headless test runs require a browser binary (Firefox/Chrome) available on PATH. On CI, you may need to install the browser or enable Xvfb.

## Minimal GitHub Actions example

This example runs host tests and wasm-pack tests on the ubuntu runner. Depending on the runner image you might need to install additional packages or start Xvfb.

```yaml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Rust
        uses: dtolnay/gh-actions-rust@stable
      - name: Add wasm target
        run: rustup target add wasm32-unknown-unknown
      - name: Install wasm-pack
        run: cargo install wasm-pack
      - name: Run host tests
        run: cargo test --workspace --locked
      - name: Run wasm tests (headless firefox)
        run: |
          cd visualization
          wasm-pack test --headless --firefox
```

Adjust the CI snippet to your environment as needed.

---

If you want, I can add this README to the repo now (already added) and also (optionally) add a small GitHub Actions workflow file to `.github/workflows/ci.yml` — tell me if you want that next.
