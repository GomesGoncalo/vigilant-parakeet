// ── Chapter 9 — Nakagami-m fading model <nakagami> ──

= Nakagami-m fading model <nakagami>

This chapter describes the Nakagami-m small-scale fading model implemented in
the simulator, its mathematical basis, configuration parameters, and how it is
used to derive per-link packet-loss probabilities used by the routing and link
models.

== Background

Nakagami-m is a flexible distribution used to model the envelope of received
wireless signals under multipath fading. It generalises Rayleigh fading (m=1)
and approximates Rician and other fading behaviours for m>1. The probability
density function (PDF) of the received envelope r is:

  f_R(r) = (2 * m^m / Gamma(m) / Omega^m) * r^(2m-1) * exp(-m * r^2 / Omega)

where m >= 0.5 is the shape parameter and Omega = E[r^2] is the spread (mean
power). Larger m corresponds to less severe fading.

== Implementation in the simulator

The simulator implements the Nakagami-m model as a *distance-based outage
probability function* (`simulator::fading::nakagami_loss`). Given the physical
separation between two nodes in metres, the function computes the probability
that the instantaneous received power falls below the required SNR threshold,
integrating the Nakagami CDF over the path-loss-attenuated mean power at that
distance. This outage probability is used as the effective per-packet loss rate
for the channel, replacing (or augmenting) the static `loss` parameter from the
topology YAML.

Key configuration parameters (all optional; defaults shown):

- `m`: Nakagami shape parameter (≥ 0.5; default 2.0 — moderately stable channel). m=1 → Rayleigh; increase for stronger LOS.
- `eta`: path-loss exponent (default 2.0 free-space; use 2.7 for dense urban, 3.5 for indoor).
- `snr_0_db`: mean SNR at reference distance d₀=1 m, in dB (default 60 dB).
- `snr_thresh_db`: minimum SNR for successful reception, in dB (default 5 dB).
- `max_range_m`: hard cut-off distance; nodes beyond this are always unreachable (default 500 m).
- `latency_ms_per_100m`: distance-based latency added to give the routing metric a signal-strength proxy (default 2 ms/100 m).
- `update_ms`: how often the fading task recomputes loss for all channels (default 200 ms).

The model is evaluated at each simulation tick as node positions change,
making the effective loss rate a function of current inter-node distance.

Sampling modes

- `Periodic` (default): a background fading task recomputes a per-channel
  outage probability at `update_ms` intervals and stores it in the channel
  parameters. This is computationally inexpensive and matches the original
  periodic behaviour.

- `PerPacket`: the outage probability is sampled on a per-packet basis at
  transmit time, producing maximal temporal fidelity for short-lived links or
  experiments that require independent fading on every packet.

- `Hybrid`: per-packet sampling with a short coherence cache. The per-channel
  cache stores the last sampled loss and a timestamp; packets transmitted
  within `coherence_ms` of the cached sample reuse it to model short-term
  temporal correlation while still allowing re-sampling when the channel
  decorrelates.

RNG seeding and determinism

For reproducible experiments, the simulator supports deriving per-channel RNGs
from a base seed (see simulator arguments / env VPARAKEET_RNG_SEED). When a
seed is provided, each channel receives a deterministic StdRng instance derived
from the base seed and channel identifiers; otherwise the simulator falls back
to non-deterministic thread-local randomness. This enables repeatable per-packet
sampling while preserving non-determinism by default.

== Configuration example

```yaml
# simulator.yaml — top-level nakagami section enables the fading model
nakagami:
  enabled: true
  m: 0.9              # shape: < 1 for severe urban fading
  eta: 2.7            # path-loss exponent (dense urban)
  snr_0_db: 60.0      # mean SNR at 1 m reference distance
  snr_thresh_db: 5.0  # minimum SNR for reception
  max_range_m: 400.0
  update_ms: 200      # recompute fading every 200 ms

# When nakagami is enabled, static topology loss values are overridden
# per-packet by the distance-based outage probability.
topology:
  rsu1:
    obu1:
      latency: 10
      loss: 0.0   # overridden at runtime by Nakagami outage probability
```

== Usage and experimental guidance

- Use m < 1 to model severe fading (urban street canyons), m ~ 1 for Rayleigh,
  and m > 1 for mild fading or when a strong LOS component exists.
- Combine with IDM mobility (Chapter 10) so that changing inter-node distances
  drive dynamic outage probability, replicating realistic link fluctuations as
  vehicles approach or recede from RSUs.
- When combining with RSSI-based selection, the simulator injects the
  distance-derived SNR into the RSSI table so that OBU route selection sees
  signal strength that decreases with distance and fades with the chosen m.

Nakagami support allows the evaluation chapter to measure routing stability and
Key Exchange reliability under realistic small-scale fading regimes, bridging
the fidelity gap between static latency/loss parameters and a stochastic radio
model.