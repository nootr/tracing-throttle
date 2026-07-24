//! Field visitor for extracting span and event field values.
//!
//! This module provides a visitor implementation for the `tracing::field::Visit`
//! trait that extracts field values into a map for use in signature computation.
//!
//! ## Architecture
//!
//! The span context rate limiting feature records span fields in extensions
//! and uses those cached fields when computing event signatures.  These get called
//! from `TracingRateLimitLayer`'s `Filter` implementation:
//!
//! 1. `on_new_span()` method stores span fields in extensions using this `FieldVisitor`.
//!
//! 2. `event_enabled()` extracts the stored fields, and fields from each event,
//!    and uses them in signature computation.
//!
//! ## Usage
//!
//! ```rust,ignore
//! // Create the rate limit layer
//! let rate_limit = TracingRateLimitLayer::builder()
//!     .with_span_context_fields(vec!["user_id".to_string()])
//!     .build()
//!     .unwrap();
//!
//! let subscriber = tracing_subscriber::registry()
//!     .with(capture.with_filter(rate_limit_filter));
//! ```

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use tracing::field::{Field, Visit};

/// A visitor that extracts field values into a BTreeMap.
///
/// This visitor collects field name-value pairs from tracing events and spans,
/// storing them in a format suitable for signature computation. All values are
/// converted to strings using `Debug` formatting.
///
/// Uses `Cow<'static, str>` for zero-copy field names when possible, reducing
/// allocations during signature computation.
#[derive(Debug)]
pub(crate) struct FieldVisitor {
    fields: BTreeMap<Cow<'static, str>, Cow<'static, str>>,
}

impl FieldVisitor {
    /// Create a new field visitor.
    pub fn new() -> Self {
        Self {
            fields: BTreeMap::new(),
        }
    }

    /// Consume the visitor and return the collected fields.
    pub fn into_fields(self) -> BTreeMap<Cow<'static, str>, Cow<'static, str>> {
        self.fields
    }

    /// Get a reference to the collected fields.
    #[allow(dead_code)]
    pub fn fields(&self) -> &BTreeMap<Cow<'static, str>, Cow<'static, str>> {
        &self.fields
    }
}

impl Visit for FieldVisitor {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields
            .insert(Cow::Borrowed(field.name()), Cow::Owned(value.to_string()));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(Cow::Borrowed(field.name()), Cow::Owned(value.to_string()));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(Cow::Borrowed(field.name()), Cow::Owned(value.to_string()));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(Cow::Borrowed(field.name()), Cow::Owned(value.to_string()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(Cow::Borrowed(field.name()), Cow::Owned(value.to_string()));
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields
            .insert(Cow::Borrowed(field.name()), Cow::Owned(value.to_string()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields.insert(
            Cow::Borrowed(field.name()),
            Cow::Owned(format!("{:?}", value)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_visitor() {
        let visitor = FieldVisitor::new();
        let fields = visitor.into_fields();
        assert!(fields.is_empty());
    }

    #[test]
    fn test_visitor_fields_reference() {
        let visitor = FieldVisitor::new();
        assert!(visitor.fields().is_empty());
    }
}
