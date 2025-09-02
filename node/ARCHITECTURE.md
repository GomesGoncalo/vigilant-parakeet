# node crate — architecture

Purpose: binary for running one node (instantiates NodeLib components and starts I/O tasks).

```mermaid
flowchart LR
  NodeBinary["node (binary)"] --> NodeLib["node_lib::Node (control + data)"]
  NodeBinary --> Common["common::Tun/Device"]
  NodeLib -->|uses| Common
```

Notes:
- This crate wires together `node_lib` and `common` and handles command-line args specific to running a node.
- Useful for debugging per-node behavior in isolation.
