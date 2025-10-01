use arc_swap::ArcSwap;
use mac_address::MacAddress;
use std::collections::HashMap;

/// Capacity hint for the client cache HashMap to reduce allocations.
/// Most network topologies have relatively few clients per node.
const INITIAL_CAPACITY: usize = 16;

#[derive(Default)]
pub struct ClientCache {
    cache: ArcSwap<HashMap<MacAddress, MacAddress>>,
}

impl ClientCache {
    /// Create a new ClientCache with pre-allocated capacity.
    pub fn new() -> Self {
        Self {
            cache: ArcSwap::from_pointee(HashMap::with_capacity(INITIAL_CAPACITY)),
        }
    }

    /// Store a client-to-node MAC address mapping.
    /// Uses arc-swap's RCU pattern to minimize clone operations.
    pub fn store_mac(&self, client: MacAddress, node: MacAddress) {
        // Fast path: check if the mapping already exists and is unchanged
        {
            let current = self.cache.load();
            if let Some(&existing_node) = current.get(&client) {
                if existing_node == node {
                    // Mapping already exists with same value, no update needed
                    return;
                }
            }
        }

        // Use RCU (Read-Copy-Update) pattern for efficient update
        self.cache.rcu(|old| {
            let mut new = HashMap::clone(old);
            new.insert(client, node);
            new
        });
    }

    /// Get the node MAC address for a given client MAC address.
    pub fn get(&self, client: MacAddress) -> Option<MacAddress> {
        let cache = self.cache.load();
        cache.get(&client).copied()
    }

    /// Get the current number of cached entries.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.cache.load().len()
    }

    /// Check if the cache is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.cache.load().is_empty()
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

    #[test]
    fn new_cache_has_preallocated_capacity() {
        let cache = ClientCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn updating_existing_mapping() {
        let cache = ClientCache::new();
        let client: MacAddress = [5u8; 6].into();
        let node1: MacAddress = [6u8; 6].into();
        let node2: MacAddress = [7u8; 6].into();

        // Store initial mapping
        cache.store_mac(client, node1);
        assert_eq!(cache.get(client), Some(node1));

        // Update to new node
        cache.store_mac(client, node2);
        assert_eq!(cache.get(client), Some(node2));

        // Length should still be 1 (update, not insert)
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn multiple_clients() {
        let cache = ClientCache::new();
        let clients: Vec<MacAddress> = (0..5).map(|i| [i, i, i, i, i, i].into()).collect();
        let node: MacAddress = [0xaa; 6].into();

        // Store multiple clients
        for &client in &clients {
            cache.store_mac(client, node);
        }

        assert_eq!(cache.len(), 5);

        // Verify all clients are cached
        for &client in &clients {
            assert_eq!(cache.get(client), Some(node));
        }
    }

    #[test]
    fn concurrent_reads_while_updating() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(ClientCache::new());
        let client: MacAddress = [0x10; 6].into();
        let node: MacAddress = [0x20; 6].into();

        // Initial insert
        cache.store_mac(client, node);

        // Spawn reader threads
        let mut handles = vec![];
        for _ in 0..4 {
            let cache_clone = Arc::clone(&cache);
            let handle = thread::spawn(move || {
                for _ in 0..100 {
                    let result = cache_clone.get(client);
                    assert_eq!(result, Some(node));
                }
            });
            handles.push(handle);
        }

        // Update in main thread while readers are running
        for i in 0..10 {
            let new_client: MacAddress = [0x30 + i; 6].into();
            cache.store_mac(new_client, node);
        }

        // Wait for readers
        for handle in handles {
            handle.join().unwrap();
        }

        assert!(cache.len() >= 11); // original + 10 new
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let cache = ClientCache::new();
        let client: MacAddress = [0xff; 6].into();
        assert_eq!(cache.get(client), None);
    }
}
