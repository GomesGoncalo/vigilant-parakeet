#[cfg(not(target_family = "wasm"))]
use config::Value;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelParameters {
    pub latency: Duration,
    pub loss: f64,
    /// Jitter adds random variation to latency: ±jitter_ms around base latency
    /// For example, latency=10ms, jitter=2ms gives range [8ms, 12ms]
    pub jitter: Duration,
}

#[cfg(not(target_family = "wasm"))]
impl From<HashMap<String, Value>> for ChannelParameters {
    fn from(param: HashMap<String, Value>) -> Self {
        let latency = match param.get("latency") {
            Some(val) => val.clone().into_uint().unwrap_or(0),
            None => 0,
        };
        let loss = match param.get("loss") {
            Some(val) => val.clone().into_float().unwrap_or(0.0),
            None => 0.0,
        };
        let jitter = match param.get("jitter") {
            Some(val) => val.clone().into_uint().unwrap_or(0),
            None => 0,
        };

        Self {
            latency: Duration::from_millis(latency),
            loss,
            jitter: Duration::from_millis(jitter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChannelParameters;
    use config::Value;
    use std::collections::HashMap;
    use std::time::Duration;

    #[test]
    fn channel_parameters_from_map_parses_values() {
        let mut m: HashMap<String, Value> = HashMap::new();
        m.insert("latency".to_string(), Value::from(150u64));
        m.insert("loss".to_string(), Value::from(0.125f64));
        m.insert("jitter".to_string(), Value::from(10u64));

        let cp = ChannelParameters::from(m);
        assert_eq!(cp.latency, Duration::from_millis(150));
        assert!((cp.loss - 0.125).abs() < f64::EPSILON);
        assert_eq!(cp.jitter, Duration::from_millis(10));
    }

    #[test]
    fn channel_parameters_missing_keys_defaults() {
        let m: HashMap<String, Value> = HashMap::new();
        let cp = ChannelParameters::from(m);
        assert_eq!(cp.latency, Duration::from_millis(0));
        assert_eq!(cp.loss, 0.0);
        assert_eq!(cp.jitter, Duration::from_millis(0));
    }
}
