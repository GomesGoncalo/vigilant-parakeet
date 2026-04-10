// ── Abstract ──────────────────────────────────────────────────────────────────

Vehicular networks present unique challenges for routing protocol design: nodes
are highly mobile, connectivity is intermittent, and security requirements are
stringent. This work documents the design, implementation, and evaluation of
*vigilant-parakeet*, a Rust-based simulator and visualiser for experimenting
with Layer‑3 routing protocols in vehicular environments composed of On‑Board
Units (OBUs) and Road‑Side Units (RSUs).

The simulator leverages Linux network namespaces to provide realistic,
isolated per-node network stacks without requiring physical hardware. The
implemented routing stack is heartbeat-driven and includes a revised route
selection algorithm with a latency-aware composite scoring metric,
RSSI-aware candidate ranking, a 30% hysteresis band to reduce flapping, and
N‑best candidate caching for fast, stable failover. RSSI measurements are
injected via a shared RSSI table and applied through a 3 dB switch margin so
that the next-hop with the strongest first-hop signal is preferred; a
reception-quality/hops fallback operates
when RSSI is unavailable. The physical layer model
includes a Nakagami‑m small‑scale fading implementation that maps outage
probability to per-link loss as a function of inter-node distance, enabling
controlled evaluation of fading effects on higher-layer behaviour. Mobility is
improved with OSM-driven trajectories and an integrated Intelligent Driver
Model (IDM) for realistic longitudinal and lane-change dynamics.

Security features remain comprehensive: configurable classical and
post‑quantum KEMs and signature schemes, HKDF-derived session keys, optional
PKI or TOFU provisioning modes, and a signed session-revocation mechanism.
The browser-based visualisation dashboard provides rAF-driven marker
interpolation, canvas overlays for directional arrows, RSSI sparklines and
N‑best/KE popups, and a JS-native high-frequency polling path to bypass WASM
for performance-sensitive updates.

Contributions: a modular Rust workspace and test harness; a routing algorithm
rework (RSSI-based next-hop and source selection with 3 dB hysteresis,
quality/hops fallback, N‑best); Nakagami‑m distance-based
fading and its integration into the evaluation pipeline; OSM+IDM mobility
integration; and a substantial enhancement of visualization and observability
tooling. The code, experiment plans, and produced artifacts accompany this
thesis to support reproducible research.