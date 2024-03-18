use std::{collections::HashMap, sync::RwLock};

use mac_address::MacAddress;

#[derive(Default)]
pub struct ClientCache {
    cache: RwLock<HashMap<MacAddress, MacAddress>>,
}

impl ClientCache {
    pub fn store_mac(&self, client: MacAddress, node: MacAddress) {
        self.cache.write().unwrap().insert(client, node);
    }

    pub fn get(&self, client: MacAddress) -> Option<MacAddress> {
        self.cache.read().unwrap().get(&client).copied()
    }
}
