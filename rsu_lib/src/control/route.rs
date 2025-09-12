use std::{
    fmt::{Display, Formatter, Result},
    time::Duration,
};

use mac_address::MacAddress;

#[derive(Debug)]
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

#[cfg(test)]
mod tests {
    use super::Route;
    use mac_address::MacAddress;

    #[test]
    fn route_display_contains_fields() {
        let mac: MacAddress = [1u8, 2, 3, 4, 5, 6].into();
        let r = Route {
            hops: 3,
            mac,
            latency: None,
        };
        let s = format!("{r}");
        assert!(s.contains("mac"));
        assert!(s.contains("hops: 3"));
        assert!(s.contains("latency: None"));
    }

    #[test]
    fn route_display_with_latency_some() {
        let mac: MacAddress = [10u8, 11, 12, 13, 14, 15].into();
        let r = Route {
            hops: 1,
            mac,
            latency: Some(std::time::Duration::from_millis(5)),
        };
        let s = format!("{r}");
        assert!(s.contains("latency: Some"));
        assert!(s.contains("hops: 1"));
    }
}
