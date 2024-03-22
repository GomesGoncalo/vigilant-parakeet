use serde::{Deserialize, Serialize};

#[derive(Serialize, Default, Clone, Copy, Debug, Deserialize)]
pub struct Stats {
    pub received_packets: u128,
    pub received_bytes: u128,
    pub transmitted_packets: u128,
    pub transmitted_bytes: u128,
}
