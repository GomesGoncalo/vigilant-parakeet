use arc_swap::ArcSwap;
use mac_address::MacAddress;
use std::collections::HashMap;

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

#[cfg(test)]
mod tests {
    use super::ClientCache;
    use mac_address::MacAddress;

    #[test]
    fn store_and_get_mac() {
        let cache = ClientCache::default();
        let client: MacAddress = [1u8; 6].into();
        let node: MacAddress = [2u8; 6].into();
        assert!(cache.get(client).is_none());
        cache.store_mac(client, node);
        let got = cache.get(client);
        assert_eq!(got, Some(node));
    }

    #[test]
    fn storing_same_mapping_is_noop() {
        let cache = ClientCache::default();
        let client: MacAddress = [3u8; 6].into();
        let node: MacAddress = [4u8; 6].into();
        cache.store_mac(client, node);
        // store same mapping again; should not panic and should still return same value
        cache.store_mac(client, node);
        assert_eq!(cache.get(client), Some(node));
    }
}
