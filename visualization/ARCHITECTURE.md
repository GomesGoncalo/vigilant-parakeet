# visualization crate — architecture

Purpose: web-based visualization of simulation state (consumes `/node_info` from simulator).

```mermaid
flowchart LR
  Browser["browser UI"] -->|fetches| API["simulator /node_info"]
  API -->|serves| Graph["graph renderer (JS / Rust wasm)"]
  Graph -->|renders| Layout["Plotly / D3 / CSS layout"]
```

Notes:
- The web UI normalizes node metadata (type, upstream/downstream) and draws a top-level graph.
- `visualization/src/` contains helpers to convert node state to a layout for the client.
