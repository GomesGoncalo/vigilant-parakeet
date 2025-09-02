# Repository architecture (high level)

This document gives a concise overview of the main crates and how they interact.

```mermaid
flowchart LR
  Simulator["simulator (binary)"] -->|spawns/controls| NodeBinary["node (binary)"]
  NodeBinary -->|links| NodeLib["node_lib (library)\n(control, data, messages)"]
  NodeLib -->|uses| Common["common (library)\n(device, tun, network)"]
  Simulator -->|exposes| Visualization["visualization (web UI)"]
  Visualization -->|fetches| Simulator
  NodeLib -->|exports| Messages["messages (wire formats)"]

  classDef crate fill:#f8f9fa,stroke:#333,stroke-width:1px;
  class Simulator,NodeBinary,NodeLib,Common,Visualization,Messages crate;
```

Files / crates:
- `simulator/` - simulation runtime (creates nodes, orchestrates netns).
- `node/` - thin binary that runs nodes using `node_lib`.
- `node_lib/` - core node logic: control plane, data plane, message formats.
- `common/` - shared utilities (tun, device, network interface helpers).
- `visualization/` - web UI consuming simulation state (node_info endpoint).

Tips:
- Each crate contains its own `ARCHITECTURE.md` with focused details.
- Use these diagrams when debugging routing/control interactions or extending modules.
