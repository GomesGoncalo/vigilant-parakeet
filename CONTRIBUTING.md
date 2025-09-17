Thank you for contributing to Vigilant Parakeet!

This guide explains how to set up the project for development, run tests and coverage locally, and follow the repository conventions so your changes pass CI.

## Quick start (local)

1. Install Rust toolchain:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
rustup component add rustfmt clippy
```

2. Clone and enter the repo:

```bash
git clone <repo-url>
cd vigilant-parakeet
```

3. Install project tooling (optional):

```bash
cargo install cargo-tarpaulin --locked   # for coverage
```

4. Build and run tests quickly:

```bash
# Build workspace
cargo build --workspace
# Run workspace tests with test helper shims enabled
cargo test --workspace --features test_helpers
```

## Formatting and linting

This repository enforces code style and lints in CI. Please run these before opening a PR:

```bash
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

If CI fails for formatting or clippy, fix locally and push again.

## Commit messages

Use Conventional Commits. Example:

```
feat(node_lib): add heartbeat retry mechanism

- What: add exponential backoff to heartbeat sends
- Why: improve reliability under lossy links
- How: add RetryConfig, update sender
- Testing: cargo test -p node_lib and cargo test --workspace
```

CI runs hooks that may enforce this format.

## Tests and test helpers

- Many tests use a `test_helpers` feature to provide non-privileged test shims for TUN/device operations. Run tests with this feature in CI-like runs:

```bash
cargo test --workspace --features test_helpers
```

- For deterministic timing tests the code uses `tokio::time::pause()` and `advance()` with `#[tokio::test(flavor = "current_thread")]`.

## Coverage (tarpaulin)

We collect coverage in CI over library crates only. Locally you can run:

```bash
cargo tarpaulin -p common -p node_lib -p obu_lib -p rsu_lib --features test_helpers --out Lcov --timeout 180 --run-types Tests --output-dir ./target/tarpaulin
```

Note: `simulator` is excluded from tarpaulin because it requires network namespaces and root privileges and is not suited for coverage collection in CI.

## CI and PR workflow

- Push branches and open PRs against `main`.
- CI will run build, tests, clippy, fmt checks and generate coverage badges.
- Ensure all checks pass before requesting a review.

## Developing features and tests

- Run `cargo test -p <crate>` during development for faster feedback.
- When adding tests that would normally require root or network namespaces, use the `test_helpers` shims or construct minimal in-memory instances to avoid spawning background tasks that do privileged work.

## Making small contributions

- Open an issue describing the change or bug if it's non-trivial.
- Create a branch named `feat/<short-desc>` or `fix/<short-desc>`.
- Keep changes small and add tests where applicable.

## Contact and conventions

If in doubt, open a small PR and ask for feedback in the PR description. Follow the repository's ARCHITECTURE.md files for higher-level design guidance.

Thank you for helping improve Vigilant Parakeet!
