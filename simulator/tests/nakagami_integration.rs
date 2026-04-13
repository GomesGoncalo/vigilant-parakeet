use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use simulator::fading::{sample_nakagami_loss, NakagamiConfig, NakagamiMode, NakagamiModel};

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
