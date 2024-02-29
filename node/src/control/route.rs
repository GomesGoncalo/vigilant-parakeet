use std::{
    fmt::{Display, Formatter, Result},
    time::Duration,
};

use mac_address::MacAddress;

pub struct Route {
    pub hops: u32,
    pub mac: MacAddress,
    pub latency: Option<Duration>,
}

impl Display for Route {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        write!(
            f,
            "Route {{ mac: {}, hops: {}, latency: {:?} }}",
            self.mac, self.hops, self.latency
        )
    }
}
