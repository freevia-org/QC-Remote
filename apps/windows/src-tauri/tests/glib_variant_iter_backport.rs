#![cfg(target_os = "linux")]

use glib::{prelude::*, Variant};

#[test]
fn variant_str_iter_pointer_output_is_valid_across_optimized_iterator_paths() {
    let variant = Variant::array_from_iter::<String>([
        "zero".to_string().to_variant(),
        "one".to_string().to_variant(),
        "two".to_string().to_variant(),
        "three".to_string().to_variant(),
    ]);

    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.next(), Some("zero"));
    assert_eq!(iter.nth(1), Some("two"));
    assert_eq!(iter.len(), 1);
    assert_eq!(iter.next_back(), Some("three"));
    assert_eq!(iter.next(), None);
    assert_eq!(iter.next_back(), None);

    let mut iter = variant.array_iter_str().unwrap();
    assert_eq!(iter.nth_back(1), Some("two"));
    assert_eq!(iter.next(), Some("zero"));
    assert_eq!(iter.last(), Some("one"));
}
