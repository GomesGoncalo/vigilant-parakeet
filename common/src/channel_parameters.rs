#[cfg(not(target_family = "wasm"))]
use config::Value;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

#[derive(Debug, Copy, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelParameters {
    pub latency: Duration,
    pub loss: f64,
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

        Self {
            latency: Duration::from_millis(latency),
            loss,
        }
    }
}
