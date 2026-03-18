//! Connector resolver — replaces `~~category` placeholders in plugin content.
//!
//! Some plugin templates reference connector categories like `~~research` or `~~code`.
//! This module resolves them to actual connector values at runtime.

use std::collections::HashMap;

/// Resolves `~~category` placeholders in plugin content.
pub struct ConnectorResolver {
    /// Known connector categories and their resolved values.
    connectors: HashMap<String, String>,
}

impl ConnectorResolver {
    /// Create a new resolver with the given connector mappings.
    pub fn new(connectors: HashMap<String, String>) -> Self {
        Self { connectors }
    }

    /// Create a resolver with default connector mappings.
    pub fn with_defaults() -> Self {
        let mut connectors = HashMap::new();
        connectors.insert("research".to_string(), "research connector".to_string());
        connectors.insert("code".to_string(), "code connector".to_string());
        connectors.insert("data".to_string(), "data connector".to_string());
        Self { connectors }
    }

    /// Resolve all `~~category` placeholders in the given text.
    ///
    /// Unknown categories are left unchanged.
    pub fn resolve(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (category, value) in &self.connectors {
            let placeholder = format!("~~{}", category);
            result = result.replace(&placeholder, value);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connector_resolve_category() {
        let resolver = ConnectorResolver::with_defaults();
        let input = "Use ~~research to find papers";
        let output = resolver.resolve(input);
        assert_eq!(output, "Use research connector to find papers");
    }

    #[test]
    fn test_connector_no_placeholder() {
        let resolver = ConnectorResolver::with_defaults();
        let input = "No placeholders here";
        let output = resolver.resolve(input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_connector_unknown_category() {
        let resolver = ConnectorResolver::with_defaults();
        let input = "Use ~~unknown connector";
        let output = resolver.resolve(input);
        // Unknown categories left unchanged
        assert_eq!(output, "Use ~~unknown connector");
    }

    #[test]
    fn test_connector_multiple_placeholders() {
        let resolver = ConnectorResolver::with_defaults();
        let input = "Use ~~research and ~~code together";
        let output = resolver.resolve(input);
        assert_eq!(output, "Use research connector and code connector together");
    }

    #[test]
    fn test_connector_custom_mapping() {
        let mut connectors = HashMap::new();
        connectors.insert("api".to_string(), "REST API v2".to_string());
        let resolver = ConnectorResolver::new(connectors);

        let output = resolver.resolve("Connect via ~~api");
        assert_eq!(output, "Connect via REST API v2");
    }
}
