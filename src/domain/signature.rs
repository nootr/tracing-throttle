//! Event signature computation for log deduplication.
//!
//! An event signature uniquely identifies a class of log events based on:
//! - Log level (INFO, WARN, ERROR, etc.)
//! - Message template
//! - Field names and values
//! - Optional: target, module path
//!
//! Events with the same signature are considered "duplicates" for rate limiting purposes.

use ahash::AHasher;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};

/// A unique signature identifying a class of log events.
///
/// Events with identical signatures are considered duplicates and subject to
/// the same rate limiting policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventSignature(u64);

impl EventSignature {
    /// Compute a signature from event components.
    ///
    /// # Arguments
    /// * `level` - Log level as string (e.g., "INFO", "WARN")
    /// * `message` - Message template
    /// * `fields` - Key-value pairs of structured fields (sorted for consistency)
    /// * `target` - Optional target name
    ///
    /// # Performance
    /// This function is designed for the hot path:
    /// - Uses fast ahash algorithm
    /// - O(1) complexity for signature creation after hashing
    /// - No allocations
    pub fn new(
        level: &str,
        message: &str,
        fields: &BTreeMap<String, String>,
        target: Option<&str>,
    ) -> Self {
        let mut hasher = AHasher::default();

        // Hash level
        level.hash(&mut hasher);

        // Hash message
        message.hash(&mut hasher);

        // Hash fields in sorted order (BTreeMap guarantees this)
        for (key, value) in fields {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
        }

        // Hash target if present
        if let Some(t) = target {
            t.hash(&mut hasher);
        }

        EventSignature(hasher.finish())
    }

    /// Create a signature with minimal fields (level and message only).
    ///
    /// Useful for simple logging scenarios where field-based deduplication
    /// is not needed.
    pub fn simple(level: &str, message: &str) -> Self {
        let fields = BTreeMap::new();
        Self::new(level, message, &fields, None)
    }

    /// Get the raw hash value.
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for EventSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_events_produce_same_signature() {
        let fields1 = BTreeMap::from([
            ("user".to_string(), "alice".to_string()),
            ("action".to_string(), "login".to_string()),
        ]);

        let fields2 = BTreeMap::from([
            ("user".to_string(), "alice".to_string()),
            ("action".to_string(), "login".to_string()),
        ]);

        let sig1 = EventSignature::new("INFO", "User logged in", &fields1, Some("auth"));
        let sig2 = EventSignature::new("INFO", "User logged in", &fields2, Some("auth"));

        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_different_levels_produce_different_signatures() {
        let fields = BTreeMap::new();

        let sig1 = EventSignature::new("INFO", "Message", &fields, None);
        let sig2 = EventSignature::new("WARN", "Message", &fields, None);

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_different_messages_produce_different_signatures() {
        let fields = BTreeMap::new();

        let sig1 = EventSignature::new("INFO", "Message A", &fields, None);
        let sig2 = EventSignature::new("INFO", "Message B", &fields, None);

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_different_field_values_produce_different_signatures() {
        let fields1 = BTreeMap::from([("user".to_string(), "alice".to_string())]);

        let fields2 = BTreeMap::from([("user".to_string(), "bob".to_string())]);

        let sig1 = EventSignature::new("INFO", "Message", &fields1, None);
        let sig2 = EventSignature::new("INFO", "Message", &fields2, None);

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_field_order_independence() {
        // BTreeMap ensures sorted order, but let's verify
        let mut fields1 = BTreeMap::new();
        fields1.insert("z".to_string(), "1".to_string());
        fields1.insert("a".to_string(), "2".to_string());

        let mut fields2 = BTreeMap::new();
        fields2.insert("a".to_string(), "2".to_string());
        fields2.insert("z".to_string(), "1".to_string());

        let sig1 = EventSignature::new("INFO", "Message", &fields1, None);
        let sig2 = EventSignature::new("INFO", "Message", &fields2, None);

        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_simple_signature() {
        let sig = EventSignature::simple("INFO", "Simple message");
        assert!(sig.as_u64() > 0);
    }

    #[test]
    fn test_display_format() {
        let sig = EventSignature::simple("INFO", "Test");
        let display = format!("{}", sig);
        assert_eq!(display.len(), 16); // 16 hex digits
    }

    // Edge case tests
    #[test]
    fn test_empty_message() {
        let sig = EventSignature::simple("INFO", "");
        assert!(sig.as_u64() > 0);
    }

    #[test]
    fn test_empty_target() {
        let fields = BTreeMap::new();
        let sig = EventSignature::new("INFO", "Message", &fields, Some(""));
        assert!(sig.as_u64() > 0);
    }

    #[test]
    fn test_none_target() {
        let fields = BTreeMap::new();
        let sig1 = EventSignature::new("INFO", "Message", &fields, None);
        let sig2 = EventSignature::new("INFO", "Message", &fields, Some("target"));

        // Should be different with/without target
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_large_number_of_fields() {
        let mut fields = BTreeMap::new();
        for i in 0..100 {
            fields.insert(format!("field{}", i), format!("value{}", i));
        }

        let sig = EventSignature::new("INFO", "Message", &fields, None);
        assert!(sig.as_u64() > 0);
    }

    #[test]
    fn test_same_message_different_levels() {
        let fields = BTreeMap::new();
        let sig1 = EventSignature::new("INFO", "Message", &fields, None);
        let sig2 = EventSignature::new("WARN", "Message", &fields, None);
        let sig3 = EventSignature::new("ERROR", "Message", &fields, None);

        // All should be different
        assert_ne!(sig1, sig2);
        assert_ne!(sig2, sig3);
        assert_ne!(sig1, sig3);
    }

    #[test]
    fn test_unicode_in_message() {
        let sig1 = EventSignature::simple("INFO", "Hello 世界");
        let sig2 = EventSignature::simple("INFO", "Hello 世界");
        let sig3 = EventSignature::simple("INFO", "Hello World");

        assert_eq!(sig1, sig2);
        assert_ne!(sig1, sig3);
    }

    #[test]
    fn test_special_characters() {
        let sig1 = EventSignature::simple("INFO", "Message with\nnewlines\tand\ttabs");
        let sig2 = EventSignature::simple("INFO", "Message with\nnewlines\tand\ttabs");

        assert_eq!(sig1, sig2);
    }
}
