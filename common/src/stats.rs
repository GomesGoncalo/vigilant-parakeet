use serde::{Deserialize, Serialize};

#[derive(Serialize, Default, Clone, Copy, Debug, Deserialize)]
pub struct Stats {
    pub received_packets: u128,
    pub received_bytes: u128,
    pub transmitted_packets: u128,
    pub transmitted_bytes: u128,
}

#[cfg(test)]
mod tests {
    use super::Stats;

    #[test]
    fn stats_default_is_zeroed() {
        let s = Stats::default();
        assert_eq!(s.received_packets, 0);
        assert_eq!(s.transmitted_bytes, 0);
    }
}
