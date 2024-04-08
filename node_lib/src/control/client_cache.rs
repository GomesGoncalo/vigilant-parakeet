use arc_swap::ArcSwap;
use libc::posix_spawnattr_setsigdefault;
use mac_address::MacAddress;
use std::{collections::HashMap, sync::RwLock};

#[derive(Default)]
pub struct ClientCache {
    cache: ArcSwap<HashMap<MacAddress, MacAddress>>,
}

impl ClientCache {
    pub fn store_mac(&self, client: MacAddress, node: MacAddress) {
        let result = self.get(client);
        match result {
            Some(x) if x == node => {
                return;
            }
            _ => {}
        }
        
        let cache = self.cache.load();
        let mut cache = (**cache).clone();
        cache.insert(client, node);
        self.cache.store(cache.into());
    }

    pub fn get(&self, client: MacAddress) -> Option<MacAddress> {
        let cache = self.cache.load();
        cache.get(&client).copied()
    }
}
