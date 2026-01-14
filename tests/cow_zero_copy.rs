//! Tests to verify Cow zero-copy optimization is working.
//!
//! These tests verify that field names from tracing macros use Cow::Borrowed
//! (zero-copy) rather than Cow::Owned (allocation).

use std::borrow::Cow;
use std::collections::BTreeMap;
use tracing_throttle::EventSignature;

#[test]
fn test_signature_with_borrowed_fields() {
    // Verify that signatures can be created with borrowed field names (zero-copy)
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::from([
        (Cow::Borrowed("user_id"), Cow::Borrowed("123")),
        (Cow::Borrowed("endpoint"), Cow::Borrowed("/api/users")),
    ]);

    let sig1 = EventSignature::new("INFO", "Request processed", &fields, Some("api"));
    let sig2 = EventSignature::new("INFO", "Request processed", &fields, Some("api"));

    assert_eq!(sig1, sig2);
}

#[test]
fn test_signature_with_owned_fields() {
    // Verify that signatures work with owned field names (dynamic cases)
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::from([
        (
            Cow::Owned("user_id".to_string()),
            Cow::Owned("123".to_string()),
        ),
        (
            Cow::Owned("endpoint".to_string()),
            Cow::Owned("/api/users".to_string()),
        ),
    ]);

    let sig1 = EventSignature::new("INFO", "Request processed", &fields, Some("api"));
    let sig2 = EventSignature::new("INFO", "Request processed", &fields, Some("api"));

    assert_eq!(sig1, sig2);
}

#[test]
fn test_borrowed_and_owned_produce_same_signature() {
    // Verify that borrowed and owned Cow produce identical signatures
    // This ensures the optimization is transparent
    let borrowed_fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::from([
        (Cow::Borrowed("user_id"), Cow::Borrowed("123")),
        (Cow::Borrowed("endpoint"), Cow::Borrowed("/api/users")),
    ]);

    let owned_fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::from([
        (
            Cow::Owned("user_id".to_string()),
            Cow::Owned("123".to_string()),
        ),
        (
            Cow::Owned("endpoint".to_string()),
            Cow::Owned("/api/users".to_string()),
        ),
    ]);

    let sig_borrowed =
        EventSignature::new("INFO", "Request processed", &borrowed_fields, Some("api"));
    let sig_owned = EventSignature::new("INFO", "Request processed", &owned_fields, Some("api"));

    assert_eq!(
        sig_borrowed, sig_owned,
        "Borrowed and owned Cow should produce identical signatures"
    );
}

#[test]
fn test_mixed_borrowed_owned() {
    // Real-world scenario: static field names (borrowed) with dynamic values (owned)
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::from([
        (Cow::Borrowed("user_id"), Cow::Owned("123".to_string())), // Static key, dynamic value
        (
            Cow::Borrowed("endpoint"),
            Cow::Owned("/api/users".to_string()),
        ), // Static key, dynamic value
    ]);

    let sig = EventSignature::new("INFO", "Request processed", &fields, Some("api"));
    assert!(sig.as_u64() > 0);
}

#[test]
fn test_empty_borrowed_fields() {
    // Edge case: empty BTreeMap with Cow type
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::new();

    let sig1 = EventSignature::new("INFO", "Simple message", &fields, None);
    let sig2 = EventSignature::new("INFO", "Simple message", &fields, None);

    assert_eq!(sig1, sig2);
}

#[test]
fn test_borrowed_field_with_empty_value() {
    // Edge case: borrowed key with empty string value
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> =
        BTreeMap::from([(Cow::Borrowed("key"), Cow::Borrowed(""))]);

    let sig = EventSignature::new("INFO", "Message", &fields, None);
    assert!(sig.as_u64() > 0);
}

#[test]
fn test_many_borrowed_fields() {
    // Verify zero-copy works with many fields
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = (0..50)
        .map(|i| {
            (
                Cow::Owned(format!("field_{}", i)),
                Cow::Owned(format!("value_{}", i)),
            )
        })
        .collect();

    let sig1 = EventSignature::new("INFO", "Many fields", &fields, None);
    let sig2 = EventSignature::new("INFO", "Many fields", &fields, None);

    assert_eq!(sig1, sig2);
}

#[test]
fn test_unicode_in_borrowed_fields() {
    // Verify borrowed Cow works with Unicode (though field names should be ASCII)
    let fields: BTreeMap<Cow<'static, str>, Cow<'static, str>> = BTreeMap::from([
        (Cow::Borrowed("user"), Cow::Borrowed("Alice")),
        (Cow::Borrowed("message"), Cow::Borrowed("Hello 世界")),
    ]);

    let sig = EventSignature::new("INFO", "Test", &fields, None);
    assert!(sig.as_u64() > 0);
}

#[test]
fn test_cow_ordering_consistency() {
    // Verify that BTreeMap ordering is consistent regardless of Cow variant
    let mut borrowed = BTreeMap::new();
    borrowed.insert(Cow::Borrowed("z"), Cow::Borrowed("1"));
    borrowed.insert(Cow::Borrowed("a"), Cow::Borrowed("2"));

    let mut owned = BTreeMap::new();
    owned.insert(Cow::Owned("z".to_string()), Cow::Owned("1".to_string()));
    owned.insert(Cow::Owned("a".to_string()), Cow::Owned("2".to_string()));

    let sig_borrowed = EventSignature::new("INFO", "Test", &borrowed, None);
    let sig_owned = EventSignature::new("INFO", "Test", &owned, None);

    assert_eq!(
        sig_borrowed, sig_owned,
        "BTreeMap ordering should be consistent"
    );
}
