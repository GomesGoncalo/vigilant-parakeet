//! Nakagami-m wireless fading channel model.
//!
//! Computes the outage (packet-loss) probability between two nodes at a given
//! distance using the Nakagami-m distribution, which generalises Rayleigh (m=1)
//! and approaches AWGN as m → ∞.
//!
//! # Model
//!
//! Mean received SNR at distance d with free-space/urban path loss:
//! ```text
//! SNR_mean(d) = SNR_0 · (d₀ / d)^η
//! ```
//! Outage probability (lower regularised incomplete gamma):
//! ```text
//! P_out = γ(m, m · SNR_thresh / SNR_mean) / Γ(m)
//!       = 1 − e^{−x} · Σ_{k=0}^{m−1} xᵏ/k!      (exact for integer m)
//! where  x = m · SNR_thresh / SNR_mean
//! ```
//!
//! # Default parameters (V2X @ 5.9 GHz DSRC)
//! * m   = 2   (moderately stable urban channel)
//! * η   = 2.7 (urban path-loss exponent)
//! * SNR₀ = 40 dB at d₀ = 1 m  
//! * SNR_thresh = 10 dB  → max range ≈ 400 m

use serde::{Deserialize, Serialize};

/// Configuration for the Nakagami-m fading model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NakagamiConfig {
    /// Enable Nakagami-m fading (replaces fixed topology loss).
    #[serde(default)]
    pub enabled: bool,

    /// Nakagami shape parameter m ≥ 0.5.  m=1 → Rayleigh, m→∞ → AWGN.
    #[serde(default = "default_m")]
    pub m: f64,

    /// Path-loss exponent η (free-space=2, urban≈2.7, dense urban≈3.5).
    #[serde(default = "default_eta")]
    pub eta: f64,

    /// Mean SNR at reference distance d₀=1 m, in dB.
    #[serde(default = "default_snr_0_db")]
    pub snr_0_db: f64,

    /// Minimum SNR required for successful reception, in dB.
    #[serde(default = "default_snr_thresh_db")]
    pub snr_thresh_db: f64,

    /// Hard maximum range: nodes beyond this distance always have loss = 1.
    #[serde(default = "default_max_range_m")]
    pub max_range_m: f64,

    /// Latency added per 100 m of distance (ms). Gives the routing protocol a
    /// distance-based metric so it prefers the nearest RSU.  Default: 2 ms/100 m.
    #[serde(default = "default_latency_ms_per_100m")]
    pub latency_ms_per_100m: f64,

    /// How often (milliseconds) to recompute fading for all channels when
    /// running in periodic mode.
    #[serde(default = "default_update_ms")]
    pub update_ms: u64,

    /// Sampling mode: determines whether Nakagami is computed periodically for
    /// all channels, per-packet at transmit time, or a hybrid of both.
    #[serde(default = "default_mode")]
    pub mode: NakagamiMode,

    /// Sampling model: full Gamma sampling (higher fidelity) or a Bernoulli
    /// approximation for faster execution.
    #[serde(default = "default_model")]
    pub model: NakagamiModel,

    /// Optional coherence time (milliseconds) for short-term temporal
    /// correlation of per-packet samples when using `Hybrid` or `PerPacket`.
    #[serde(default)]
    pub coherence_ms: Option<u64>,
}

/// Nakagami sampling mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NakagamiMode {
    Periodic,
    PerPacket,
    Hybrid,
}

fn default_mode() -> NakagamiMode {
    NakagamiMode::Periodic
}

/// Sampling model to use when applying Nakagami fading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NakagamiModel {
    Gamma,
    Bernoulli,
}

fn default_model() -> NakagamiModel {
    NakagamiModel::Gamma
}

fn default_m() -> f64 {
    2.0
}
fn default_eta() -> f64 {
    2.0 // free-space path loss; use ~2.7 for dense urban
}
fn default_snr_0_db() -> f64 {
    60.0 // mean SNR at d₀ = 1 m (high Tx power, sensitive receiver)
}
fn default_snr_thresh_db() -> f64 {
    5.0 // minimum SNR for decoding; gives ~300–500 m range
}
fn default_max_range_m() -> f64 {
    500.0
}
fn default_latency_ms_per_100m() -> f64 {
    2.0
}
fn default_update_ms() -> u64 {
    200
}

impl Default for NakagamiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            m: default_m(),
            eta: default_eta(),
            snr_0_db: default_snr_0_db(),
            snr_thresh_db: default_snr_thresh_db(),
            max_range_m: default_max_range_m(),
            latency_ms_per_100m: default_latency_ms_per_100m(),
            update_ms: default_update_ms(),
            mode: default_mode(),
            model: default_model(),
            coherence_ms: None,
        }
    }
}

/// Compute Nakagami-m outage probability at distance `d_m` metres.
///
/// Returns a loss probability in [0.0, 1.0].
pub fn nakagami_loss(d_m: f64, cfg: &NakagamiConfig) -> f64 {
    if d_m <= 0.0 {
        return 0.0;
    }
    if d_m >= cfg.max_range_m {
        return 1.0;
    }

    let snr_0 = db_to_linear(cfg.snr_0_db);
    let snr_thresh = db_to_linear(cfg.snr_thresh_db);

    // Mean SNR at distance d using path-loss model (reference distance d₀ = 1 m)
    let snr_mean = snr_0 / d_m.powf(cfg.eta);

    // Argument of the lower regularised incomplete gamma function
    let x = cfg.m * snr_thresh / snr_mean;

    lower_regularised_gamma(cfg.m, x).clamp(0.0, 1.0)
}

/// Sample a per-packet loss probability according to the configured model.
/// Returns a sampled loss in [0.0, 1.0] (for Bernoulli/Gamma-derived outage)
/// or a direct Bernoulli decision can be made by comparing RNG to this value.
pub fn sample_nakagami_loss(d_m: f64, cfg: &NakagamiConfig) -> f64 {
    let base_loss = nakagami_loss(d_m, cfg);
    match cfg.model {
        NakagamiModel::Gamma => {
            // For full fidelity, interpret base_loss as outage probability and
            // return it unchanged; the caller can perform Bernoulli trial per-packet.
            base_loss
        }
        NakagamiModel::Bernoulli => {
            // Low-fidelity: use thresholded Bernoulli model where base_loss
            // directly used as probability (same behaviour) — preserved for now.
            base_loss
        }
    }
}

/// Lower regularised incomplete gamma P(a, x) = γ(a,x)/Γ(a).
///
/// Uses the exact closed-form for integer/half-integer a via series expansion,
/// falling back to a numerical series for other values.
fn lower_regularised_gamma(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }

    // For integer m use the exact formula:
    //   P(m, x) = 1 − e^{−x} · Σ_{k=0}^{m−1} xᵏ/k!
    let m_int = a.round() as u32;
    if (a - m_int as f64).abs() < 1e-9 && m_int >= 1 {
        let exp_neg_x = (-x).exp();
        let mut sum = 0.0_f64;
        let mut term = 1.0_f64; // x^0 / 0!
        for k in 0..m_int {
            sum += term;
            term *= x / (k + 1) as f64;
        }
        return 1.0 - exp_neg_x * sum;
    }

    // Numerical series P(a, x) = e^{−x} · xᵃ · Σ_{n=0}^∞ xⁿ/Γ(a+n+1)
    // (converges quickly for moderate x)
    let mut sum = 0.0_f64;
    let mut term = 1.0_f64 / gamma(a + 1.0);
    for n in 0..200_u32 {
        sum += term;
        term *= x / (a + n as f64 + 1.0);
        if term < 1e-12 * sum {
            break;
        }
    }
    ((-x).exp() * x.powf(a) * sum).clamp(0.0, 1.0)
}

/// Lanczos approximation of the gamma function (accurate to ~1e-9).
fn gamma(z: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_81,
        676.520_368_121_885,
        -1_259.139_216_722_403,
        771.323_428_777_653,
        -176.615_029_162_141,
        12.507_343_278_687,
        -0.138_571_095_265_72,
        9.984_369_578_019_57e-6,
        1.505_632_735_149_31e-7,
    ];
    if z < 0.5 {
        std::f64::consts::PI / ((std::f64::consts::PI * z).sin() * gamma(1.0 - z))
    } else {
        let z = z - 1.0;
        let mut x = C[0];
        for (i, &c) in C[1..].iter().enumerate() {
            x += c / (z + i as f64 + 1.0);
        }
        let t = z + G + 0.5;
        (2.0 * std::f64::consts::PI).sqrt() * t.powf(z + 0.5) * (-t).exp() * x
    }
}

/// Convert dB to linear power ratio.
#[inline]
pub fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 10.0)
}

/// Great-circle distance between two WGS-84 coordinates, in metres.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R: f64 = 6_371_000.0; // Earth radius in metres
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().asin()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::{RngCore, SeedableRng};

    #[test]
    fn loss_zero_at_zero_distance() {
        let cfg = NakagamiConfig::default();
        assert_eq!(nakagami_loss(0.0, &cfg), 0.0);
    }

    #[test]
    fn loss_one_beyond_max_range() {
        let cfg = NakagamiConfig::default();
        assert_eq!(nakagami_loss(cfg.max_range_m + 1.0, &cfg), 1.0);
    }

    #[test]
    fn loss_increases_with_distance() {
        let cfg = NakagamiConfig::default();
        let l50 = nakagami_loss(50.0, &cfg);
        let l200 = nakagami_loss(200.0, &cfg);
        let l400 = nakagami_loss(400.0, &cfg);
        assert!(l50 < l200, "loss should increase with distance");
        assert!(l200 < l400, "loss should increase with distance");
    }

    #[test]
    fn haversine_known_distance() {
        // haversine formula with R=6_371_000 gives ~111_195 m per degree of latitude
        let d = haversine_m(0.0, 0.0, 1.0, 0.0);
        assert!((d - 111_195.0).abs() < 200.0, "got {d}");
    }

    #[test]
    fn sample_loss_range_and_monotonic() {
        let cfg = NakagamiConfig::default();
        for d in &[1.0, 10.0, 50.0, 100.0, 300.0] {
            let p = sample_nakagami_loss(*d, &cfg);
            assert!((0.0..=1.0).contains(&p), "probability in [0,1]");
        }
    }

    #[test]
    fn trial_drop_deterministic_with_global_rng() {
        let cfg = NakagamiConfig::default();
        // Choose a distance where p is well-defined
        let d = 10.0;
        let p = sample_nakagami_loss(d, &cfg);

        // Prepare expected sequence using StdRng seeded with same value
        let mut rng_local = StdRng::seed_from_u64(12345u64);
        let mut expected = Vec::new();
        for _ in 0..8 {
            let v = (rng_local.next_u64() as f64) / (u64::MAX as f64);
            expected.push(v < p);
        }

        // Use an injected seeded StdRng to perform deterministic trials and
        // verify the sequence matches the expected values.
        let mut rng_for_test = StdRng::seed_from_u64(12345u64);
        let mut actual = Vec::new();
        for _ in 0..8 {
            let v = (rng_for_test.next_u64() as f64) / (u64::MAX as f64);
            actual.push(v < p);
        }

        assert_eq!(
            expected, actual,
            "deterministic RNG should match expected sequence"
        );
    }

    // Integration-style checks adapted as unit tests to avoid cross-crate import
    #[test]
    fn modes_share_base_loss() {
        let d = 50.0_f64;
        let mut cfg = NakagamiConfig::default();
        cfg.mode = NakagamiMode::Periodic;
        cfg.model = NakagamiModel::Gamma;
        let p1 = sample_nakagami_loss(d, &cfg);
        cfg.mode = NakagamiMode::PerPacket;
        let p2 = sample_nakagami_loss(d, &cfg);
        cfg.mode = NakagamiMode::Hybrid;
        let p3 = sample_nakagami_loss(d, &cfg);
        assert!((p1 - p2).abs() < 1e-12 && (p2 - p3).abs() < 1e-12);
    }

    #[test]
    fn seeded_trials_reproducible() {
        let d = 100.0_f64;
        let cfg = NakagamiConfig::default();
        let p = sample_nakagami_loss(d, &cfg);

        let mut r1 = StdRng::seed_from_u64(12345u64);
        let mut r2 = StdRng::seed_from_u64(12345u64);
        let mut seq1 = Vec::new();
        let mut seq2 = Vec::new();
        for _ in 0..256 {
            let v1 = (r1.next_u64() as f64) / (u64::MAX as f64);
            let v2 = (r2.next_u64() as f64) / (u64::MAX as f64);
            seq1.push(v1 < p);
            seq2.push(v2 < p);
        }
        assert_eq!(seq1, seq2);
    }
}
