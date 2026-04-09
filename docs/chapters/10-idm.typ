// ── Chapter 10 — IDM mobility model <idm> ──

= Intelligent Driver Model (IDM) mobility <idm>

This chapter describes the Intelligent Driver Model (IDM) based mobility
module added to the simulator. IDM is a widely-used microscopic car-following
model that produces realistic acceleration and spacing behaviours suitable for
simulating road traffic and its effect on wireless connectivity.

== IDM equations

IDM defines the longitudinal acceleration a(t) of a vehicle as:

  `a(t) = a_max * [1 - (v / v0)^delta - (s_star / s)^2]`

where:
- v is the vehicle's current speed
- v0 is the desired speed
- a_max is the comfortable acceleration
- delta is an exponent (typically 4)
- s is the current gap to the lead vehicle
- s_star is the desired minimum gap, defined as:

  `s_star = s0 + v * T + v * delta_v / (2 * sqrt(a_max * b))`

where s0 is the minimum bumper-to-bumper distance, T is the desired time gap,
delta_v is the relative speed to the lead vehicle, and b is the comfortable
braking deceleration.

== Integration and implementation

Vehicles are simulated as discrete-time agents with state (position, speed,
heading). At each simulation tick the IDM acceleration is computed and integrated
with a fixed-step integrator (Euler or semi-implicit) to update speed and
position. The mobility module exposes parameters per-vehicle or per-class so
urban, suburban and freeway behaviours can be specified.

The mobility stack hooks into the simulator's topology: each vehicle is
associated with an OBU node (one-to-one), and the vehicle's geographic
position is used to compute per-channel distances and antenna gains used by the
fading/radio model. The IDM implementation supports lane changes via a
probabilistic gap-acceptance model and a simple lateral manoeuvre scheduler,
allowing overtakes and realistic platoon formation.

== Configuration example

```yaml
mobility:
  model: idm
  dt_ms: 100
  vehicles:
    - id: v1
      obu_mac: 02:00:00:00:00:01
      params:
        v0: 15.0    # m/s (desired speed)
        a_max: 1.0
        b: 1.5
        T: 1.2
        s0: 2.0
```

== Experimental guidance

IDM enables experiments that measure the impact of realistic traffic
micro-dynamics on routing and Key Exchange reliability. Recommended studies:

- Measure route stability and failover rates at different traffic densities
  (vary vehicle count and T parameter).
- Evaluate KE success rates during lane-changes and overtakes where link
  durations are short.
- Use the Porto road-grid cache for reproducible runs.

IDM integration brings the simulator closer to real vehicular dynamics, allowing
routing experiments to reflect transient connect/disconnect events driven by
vehicle motion rather than externally injected channel updates.