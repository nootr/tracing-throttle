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
use std::borrow::Cow;
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
    /// - Zero-copy field names when static strings are used
    /// - Reduced allocations via Cow<'static, str>
    pub fn new(
        level: &str,
        message: &str,
        fields: &BTreeMap<Cow<'static, str>, Cow<'static, str>>,
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

    /// Create a signature from a raw hash value.
    ///
    /// This is useful for deserializing signatures from external storage like Redis.
    #[cfg(feature = "redis-storage")]
    pub fn from_hash(hash: u64) -> Self {
        EventSignature(hash)
    }

    /// Get the raw hash value of this signature.
    ///
    /// This is useful for serializing signatures to external storage like Redis.
    #[cfg(feature = "redis-storage")]
    pub fn as_hash(&self) -> u64 {
        self.0
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
            (Cow::Borrowed("user"), Cow::Borrowed("alice")),
            (Cow::Borrowed("action"), Cow::Borrowed("login")),
        ]);

        let fields2 = BTreeMap::from([
            (Cow::Borrowed("user"), Cow::Borrowed("alice")),
            (Cow::Borrowed("action"), Cow::Borrowed("login")),
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
        let fields1 = BTreeMap::from([(Cow::Borrowed("user"), Cow::Borrowed("alice"))]);

        let fields2 = BTreeMap::from([(Cow::Borrowed("user"), Cow::Borrowed("bob"))]);

        let sig1 = EventSignature::new("INFO", "Message", &fields1, None);
        let sig2 = EventSignature::new("INFO", "Message", &fields2, None);

        assert_ne!(sig1, sig2);
    }

    #[test]
    fn test_field_order_independence() {
        // BTreeMap ensures sorted order, but let's verify
        let mut fields1 = BTreeMap::new();
        fields1.insert(Cow::Borrowed("z"), Cow::Borrowed("1"));
        fields1.insert(Cow::Borrowed("a"), Cow::Borrowed("2"));

        let mut fields2 = BTreeMap::new();
        fields2.insert(Cow::Borrowed("a"), Cow::Borrowed("2"));
        fields2.insert(Cow::Borrowed("z"), Cow::Borrowed("1"));

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
            fields.insert(
                Cow::Owned(format!("field{}", i)),
                Cow::Owned(format!("value{}", i)),
            );
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

    // === Edge Case and Collision Resistance Tests ===

    #[test]
    fn test_null_bytes_in_strings() {
        let sig1 = EventSignature::simple("INFO", "test\0message");
        let sig2 = EventSignature::simple("INFO", "test\0message");
        let sig3 = EventSignature::simple("INFO", "testmessage");

        assert_eq!(sig1, sig2, "Null bytes should be handled consistently");
        assert_ne!(sig1, sig3, "Null bytes should affect hash");
    }

    #[test]
    fn test_zero_width_characters() {
        // Zero-width space (U+200B)
        let sig1 = EventSignature::simple("INFO", "test\u{200B}message");
        let sig2 = EventSignature::simple("INFO", "test\u{200B}message");
        let sig3 = EventSignature::simple("INFO", "testmessage");

        assert_eq!(
            sig1, sig2,
            "Zero-width characters should be included in hash"
        );
        assert_ne!(sig1, sig3, "Zero-width characters should affect hash");
    }

    #[test]
    fn test_rtl_and_bidi_text() {
        // Arabic text (RTL)
        let sig1 = EventSignature::simple("INFO", "مرحبا بك");
        let sig2 = EventSignature::simple("INFO", "مرحبا بك");
        let sig3 = EventSignature::simple("INFO", "Hello");

        assert_eq!(sig1, sig2);
        assert_ne!(sig1, sig3);
    }

    #[test]
    fn test_emoji_sequences() {
        // Emoji with skin tone modifiers
        let sig1 = EventSignature::simple("INFO", "👋🏽");
        let sig2 = EventSignature::simple("INFO", "👋🏽");
        let sig3 = EventSignature::simple("INFO", "👋"); // Without modifier

        assert_eq!(sig1, sig2);
        assert_ne!(sig1, sig3, "Emoji modifiers should affect hash");
    }

    #[test]
    fn test_combining_characters() {
        // 'e' with combining acute accent vs precomposed 'é'
        let sig1 = EventSignature::simple("INFO", "cafe\u{0301}"); // e + combining acute
        let sig2 = EventSignature::simple("INFO", "café"); // precomposed é

        // These are different Unicode representations and should hash differently
        assert_ne!(
            sig1, sig2,
            "Different Unicode normalizations should hash differently"
        );
    }

    #[test]
    fn test_very_long_message() {
        // 10K character message
        let long_msg = "a".repeat(10_000);
        let sig1 = EventSignature::simple("INFO", &long_msg);
        let sig2 = EventSignature::simple("INFO", &long_msg);

        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_very_long_field_values() {
        use std::collections::BTreeMap;

        let mut fields = BTreeMap::new();
        fields.insert(Cow::Borrowed("data"), Cow::Owned("x".repeat(10_000)));

        let sig1 = EventSignature::new("INFO", "message", &fields, Some("target"));
        let sig2 = EventSignature::new("INFO", "message", &fields, Some("target"));

        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_many_fields() {
        use std::collections::BTreeMap;

        let mut fields = BTreeMap::new();
        for i in 0..1000 {
            fields.insert(
                Cow::Owned(format!("field_{}", i)),
                Cow::Owned(format!("value_{}", i)),
            );
        }

        let sig1 = EventSignature::new("INFO", "message", &fields, Some("target"));
        let sig2 = EventSignature::new("INFO", "message", &fields, Some("target"));

        assert_eq!(sig1, sig2);
    }

    #[test]
    fn test_empty_vs_missing_strings() {
        use std::collections::BTreeMap;

        let mut fields1 = BTreeMap::new();
        fields1.insert(Cow::Borrowed("key"), Cow::Borrowed("")); // Empty value

        let fields2 = BTreeMap::new(); // Missing key

        let sig1 = EventSignature::new("INFO", "msg", &fields1, Some("target"));
        let sig2 = EventSignature::new("INFO", "msg", &fields2, Some("target"));

        assert_ne!(sig1, sig2, "Empty string should differ from missing field");
    }

    #[test]
    fn test_field_order_independence_large() {
        use std::collections::BTreeMap;

        let mut fields1 = BTreeMap::new();
        for i in 0..100 {
            fields1.insert(
                Cow::Owned(format!("z_field_{}", i)),
                Cow::Owned(format!("value_{}", i)),
            );
            fields1.insert(
                Cow::Owned(format!("a_field_{}", i)),
                Cow::Owned(format!("value_{}", i)),
            );
        }

        let mut fields2 = BTreeMap::new();
        for i in 0..100 {
            fields2.insert(
                Cow::Owned(format!("a_field_{}", i)),
                Cow::Owned(format!("value_{}", i)),
            );
            fields2.insert(
                Cow::Owned(format!("z_field_{}", i)),
                Cow::Owned(format!("value_{}", i)),
            );
        }

        let sig1 = EventSignature::new("INFO", "msg", &fields1, Some("target"));
        let sig2 = EventSignature::new("INFO", "msg", &fields2, Some("target"));

        assert_eq!(sig1, sig2, "Field insertion order should not affect hash");
    }

    #[test]
    fn test_hash_collision_resistance() {
        // Test patterns that might produce collisions in weak hash functions
        use std::collections::HashSet;

        let mut hashes = HashSet::new();
        let test_cases = vec![
            ("INFO", "test"),
            ("INFO", "tset"), // Anagram
            ("INFO", "TEST"), // Case difference
            ("IFNO", "test"), // Level anagram
            ("INFO", ""),
            ("", "test"),
            ("INFO", "test\0"),
            ("INFO", "test "), // Trailing space
            ("INFO", " test"), // Leading space
        ];

        for (level, message) in test_cases {
            let sig = EventSignature::simple(level, message);
            let hash = sig.as_u64();
            assert!(
                hashes.insert(hash),
                "Hash collision detected for ('{}', '{}')",
                level,
                message
            );
        }
    }

    #[test]
    fn test_signature_display_format() {
        let sig = EventSignature::simple("INFO", "test");
        let display = format!("{}", sig);

        // Should contain a hex representation
        assert!(!display.is_empty());
        assert!(display.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    #[cfg(feature = "redis-storage")]
    fn test_signature_from_hash_roundtrip() {
        let sig1 = EventSignature::simple("INFO", "test message");
        let hash = sig1.as_hash();
        let sig2 = EventSignature::from_hash(hash);

        assert_eq!(sig1, sig2);
        assert_eq!(sig1.as_hash(), sig2.as_hash());
    }

    #[test]
    fn test_control_characters() {
        // Test various control characters
        let sig1 = EventSignature::simple("INFO", "test\x01\x02\x03\x1F");
        let sig2 = EventSignature::simple("INFO", "test\x01\x02\x03\x1F");
        let sig3 = EventSignature::simple("INFO", "test");

        assert_eq!(sig1, sig2);
        assert_ne!(sig1, sig3);
    }

    #[test]
    fn test_whitespace_variations() {
        let sig1 = EventSignature::simple("INFO", "test message"); // space
        let sig2 = EventSignature::simple("INFO", "test\tmessage"); // tab
        let sig3 = EventSignature::simple("INFO", "test\u{00A0}message"); // non-breaking space
        let sig4 = EventSignature::simple("INFO", "test\u{2003}message"); // em space

        // All should be different
        assert_ne!(sig1, sig2);
        assert_ne!(sig1, sig3);
        assert_ne!(sig1, sig4);
        assert_ne!(sig2, sig3);
    }
}
