//! Network namespace management for node isolation.
//!
//! This module handles the lifecycle of network namespaces used to isolate
//! simulated nodes. Each namespace is automatically cleaned up when dropped.

use anyhow::{bail, Result};
use config::Value;
use netns_rs::NetNs;
use std::collections::HashMap;

/// Wrapper for network namespace with automatic cleanup on drop.
///
/// Each simulated node runs in its own network namespace, providing
/// complete network stack isolation. The namespace is automatically
/// removed when the wrapper is dropped.
pub struct NamespaceWrapper(Option<NetNs>);

impl NamespaceWrapper {
    /// Create a new namespace wrapper.
    ///
    /// # Arguments
    /// * `ns` - The NetNs instance to wrap
    pub fn new(ns: NetNs) -> Self {
        Self(Some(ns))
    }

    /// Get a reference to the inner namespace.
    pub fn inner(&self) -> Option<&NetNs> {
        self.0.as_ref()
    }
}

impl Drop for NamespaceWrapper {
    fn drop(&mut self) {
        let Some(ns) = self.0.take() else {
            panic!("No value inside?");
        };
        let _ = ns.remove();
    }
}

/// Manages network namespace creation and lifecycle.
///
/// Responsible for creating namespaces for nodes and tracking the mapping
/// between node names and namespace indices.
pub struct NamespaceManager {
    namespaces: Vec<NamespaceWrapper>,
    node_to_namespace: HashMap<String, usize>,
}

impl NamespaceManager {
    /// Create a new namespace manager.
    pub fn new() -> Self {
        Self {
            namespaces: Vec::new(),
            node_to_namespace: HashMap::new(),
        }
    }

    /// Create a namespace for a node and execute a callback within it.
    ///
    /// The callback is executed within the namespace context, allowing it to
    /// create network interfaces and configure the node's network stack.
    ///
    /// # Arguments
    /// * `node` - Node name
    /// * `node_config` - Node configuration parameters
    /// * `callback` - Function to execute within the namespace context
    ///
    /// # Returns
    /// The namespace index and the result of the callback
    pub fn create_namespace<F, T>(
        &mut self,
        node: &str,
        node_config: &HashMap<String, Value>,
        callback: F,
    ) -> Result<(usize, T)>
    where
        F: Fn(&str, &HashMap<String, Value>) -> Result<T>,
    {
        let namespace_idx = self.namespaces.len();
        let node_name = format!("sim_ns_{node}");

        let ns = NamespaceWrapper::new(NetNs::new(node_name.clone())?);
        let Some(nsi) = ns.inner() else {
            bail!("no namespace for node {}", node);
        };

        // Execute callback within namespace context
        let result = match nsi.run(|_| callback(node, node_config)) {
            Ok(Ok(res)) => res,
            Ok(Err(e)) => bail!("callback failed for node {}: {}", node, e),
            Err(e) => bail!("namespace run failed for node {}: {}", node, e),
        };

        // Track namespace
        self.namespaces.push(ns);
        self.node_to_namespace
            .insert(node.to_string(), namespace_idx);

        Ok((namespace_idx, result))
    }

    /// Get the namespace index for a node.
    #[allow(dead_code)]
    pub fn get_namespace_index(&self, node: &str) -> Option<usize> {
        self.node_to_namespace.get(node).copied()
    }

    /// Get a reference to a namespace by index.
    #[allow(dead_code)]
    pub fn get_namespace(&self, idx: usize) -> Option<&NamespaceWrapper> {
        self.namespaces.get(idx)
    }

    /// Get all namespaces.
    #[allow(dead_code)]
    pub fn namespaces(&self) -> &[NamespaceWrapper] {
        &self.namespaces
    }

    /// Get the node-to-namespace mapping.
    #[allow(dead_code)]
    pub fn node_namespace_map(&self) -> &HashMap<String, usize> {
        &self.node_to_namespace
    }

    /// Consume the manager and return the namespaces and mapping.
    pub fn into_parts(self) -> (Vec<NamespaceWrapper>, HashMap<String, usize>) {
        (self.namespaces, self.node_to_namespace)
    }
}

impl Default for NamespaceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_manager_tracks_indices() {
        let manager = NamespaceManager::new();

        // Initially empty
        assert_eq!(manager.namespaces.len(), 0);
        assert_eq!(manager.node_to_namespace.len(), 0);
    }

    #[test]
    fn namespace_manager_default() {
        let manager = NamespaceManager::default();
        assert_eq!(manager.namespaces.len(), 0);
    }

    #[test]
    fn namespace_manager_into_parts() {
        let manager = NamespaceManager::new();
        let (namespaces, mapping) = manager.into_parts();
        assert_eq!(namespaces.len(), 0);
        assert_eq!(mapping.len(), 0);
    }
}
