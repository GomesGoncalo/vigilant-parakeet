# common crate — architecture

Purpose: shared runtime helpers used by node binaries and tests (Tun, Device, network interface wrappers).

```mermaid
flowchart LR
  Tun[Tun] -->|reads/writes| Device[Device]
  Device -->|sends| NetIf[NetworkInterface]
  NetIf -->|OS network| Kernel[Kernel/TUN]
```

Main components:
- `tun.rs` — TAP/TUN helpers and buffering.
- `device.rs` — abstraction around MAC address, sending/receiving frames.
- `network_interface.rs` — helpers to configure and query network devices.

Notes:
- Kept intentionally minimal and sync-friendly so other crates can embed it easily.
