# native_viz — Native visualization for vigilant-parakeet

This crate provides a native desktop visualization that consumes the simulator HTTP API (default: http://localhost:3030).

Build

```bash
# From repository root
cargo build -p native_viz --release
```

Run

```bash
# Optional: first arg is the simulator base URL (default: http://localhost:3030)
./target/release/native_viz [http://localhost:3030]
```

Notes

- The visualizer periodically polls `/node_info` and `/metrics` endpoints and renders a live topology and per-node counters.
- The simulator ships the HTTP API (port 3030) by default; start the simulator before launching the native visualizer.
- If running the simulator on a non-localhost address, pass the simulator URL as the first argument to the visualizer binary.
