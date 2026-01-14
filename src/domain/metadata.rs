//! Event metadata for human-readable summaries.
//!
//! This module provides structures for storing event details alongside signatures,
//! allowing summaries to show what event was suppressed (not just a hash).

use std::borrow::Cow;
use std::collections::BTreeMap;

/// Metadata about a log event for human-readable summaries.
///
/// Stores the essential details needed to understand what event was suppressed
/// without having to correlate the signature hash with the original log.
///
/// Uses `Cow<'static, str>` to reduce allocations when possible, particularly
/// for field names which are often static in tracing macros.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventMetadata {
    /// Log level (e.g., "INFO", "WARN", "ERROR")
    pub level: String,
    /// Message template
    pub message: String,
    /// Target module path
    pub target: String,
    /// Structured fields (key-value pairs)
    pub fields: BTreeMap<Cow<'static, str>, Cow<'static, str>>,
}

impl EventMetadata {
    /// Create new event metadata.
    pub fn new(
        level: String,
        message: String,
        target: String,
        fields: BTreeMap<Cow<'static, str>, Cow<'static, str>>,
    ) -> Self {
        Self {
            level,
            message,
            target,
            fields,
        }
    }

    /// Format a brief description of the event for display.
    ///
    /// Returns a string like: `[ERROR] database::connection: Connection failed`
    pub fn format_brief(&self) -> String {
        format!("[{}] {}: {}", self.level, self.target, self.message)
    }

    /// Format the event with fields for detailed display.
    ///
    /// Returns a string like: `[ERROR] database::connection: Connection failed {error_code="TIMEOUT", retry_count="3"}`
    pub fn format_detailed(&self) -> String {
        if self.fields.is_empty() {
            self.format_brief()
        } else {
            let fields_str: Vec<String> = self
                .fields
                .iter()
                .map(|(k, v)| format!("{}=\"{}\"", k, v))
                .collect();
            format!("{} {{{}}}", self.format_brief(), fields_str.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_brief() {
        let metadata = EventMetadata::new(
            "ERROR".to_string(),
            "Connection failed".to_string(),
            "database::connection".to_string(),
            BTreeMap::new(),
        );

        assert_eq!(
            metadata.format_brief(),
            "[ERROR] database::connection: Connection failed"
        );
    }

    #[test]
    fn test_format_detailed_no_fields() {
        let metadata = EventMetadata::new(
            "INFO".to_string(),
            "Request processed".to_string(),
            "api::handler".to_string(),
            BTreeMap::new(),
        );

        assert_eq!(
            metadata.format_detailed(),
            "[INFO] api::handler: Request processed"
        );
    }

    #[test]
    fn test_format_detailed_with_fields() {
        let mut fields = BTreeMap::new();
        fields.insert(Cow::Borrowed("error_code"), Cow::Borrowed("TIMEOUT"));
        fields.insert(Cow::Borrowed("retry_count"), Cow::Borrowed("3"));

        let metadata = EventMetadata::new(
            "ERROR".to_string(),
            "Connection failed".to_string(),
            "database::connection".to_string(),
            fields,
        );

        let formatted = metadata.format_detailed();
        assert!(formatted.contains("[ERROR] database::connection: Connection failed"));
        assert!(formatted.contains("error_code=\"TIMEOUT\""));
        assert!(formatted.contains("retry_count=\"3\""));
    }
}
