// ── Chapter 9 — Nakagami-m fading model <nakagami> ──

= Nakagami-m fading model <nakagami>

This chapter describes the Nakagami-m small-scale fading model implemented in
the simulator, its mathematical basis, configuration parameters, and how it is
used to convert per-packet instantaneous channel states into effective link
quality metrics used by the routing and link models.

== Background

Nakagami-m is a flexible distribution used to model the envelope of received
wireless signals under multipath fading. It generalises Rayleigh fading (m=1)
and approximates Rician and other fading behaviours for m>1. The probability
density function (PDF) of the received envelope r is:

  f_R(r) = (2 * m^m / Gamma(m) / Omega^m) * r^(2m-1) * exp(-m * r^2 / Omega)

where m >= 0.5 is the shape parameter and Omega = E[r^2] is the spread (mean
power). Larger m corresponds to less severe fading.

== Implementation in the simulator

The simulator samples a Nakagami-distributed amplitude multiplier per-packet or
per-timeslot for each directed channel. The sampled amplitude is mapped to an
instantaneous power (proportional to r^2) which is converted to an instantaneous
SNR estimate using the configured transmit power / noise floor. The SNR is
then used by a simple link model to compute a packet error probability (PEP)
via a thresholded-BER model or an empirical mapping from SNR to PEP.

Two placement granularities are supported:

+ Per-packet sampling: a fresh Nakagami sample is drawn for each transmitted
  frame, modelling rapid small-scale fading appropriate for high-mobility
  scenarios and short coherence times.

+ Per-timeslot sampling: a sample is held constant across a configurable
  timeslot (e.g., 10–100 ms), modelling slower fading relative to packet rate.

Both the shape parameter m and Omega are configurable in the topology YAML for
each channel. Default values use m=1.0 (Rayleigh) and Omega derived from the
`latency`/`loss` baseline to preserve backward compatibility with static
experiments.

== Configuration example

```yaml
topology:
  rsu1:
    obu1:
      latency: 10
      loss: 0.0
      fading:
        model: nakagami
        m: 0.9
        omega: 1.0
        granularity: per-packet
```

== Usage and experimental guidance

- Use m < 1 to model severe fading (urban street canyons), m ~ 1 for Rayleigh,
  and m > 1 for mild fading or when a strong LOS component exists.
- Match sampling granularity to vehicle speed and packet rate: higher speeds and
  high packet rates justify per-packet sampling.
- When combining with RSSI-based selection, expose instantaneous RSSI samples
  averaged over a short window to the OBU to avoid basing routing decisions on
  single-sample fades.

Nakagami support allows the evaluation chapter to measure routing stability and
Key Exchange reliability under realistic small-scale fading regimes, bridging
the fidelity gap between static latency/loss parameters and a stochastic radio
model.