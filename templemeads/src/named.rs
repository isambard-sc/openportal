// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

///
/// Trait implemented by any type that can be used as a `Job` result or
/// instruction payload, giving it a stable string name that travels
/// alongside its JSON encoding (see `Job::completed<T>` / `Job::result<T>`).
///
pub trait NamedType {
    fn type_name() -> String;
}

impl NamedType for String {
    fn type_name() -> String {
        "String".to_string()
    }
}

impl NamedType for bool {
    fn type_name() -> String {
        "bool".to_string()
    }
}

///
/// Blanket impl so a domain crate never needs to hand-write `Vec<X>`
/// alongside every `X: NamedType` - `Vec<X>` is a foreign type (`Vec`) over
/// a foreign trait relative to a domain crate, so per-type impls there would
/// hit Rust's orphan rules; this one impl, local to templemeads (where the
/// trait itself lives), covers every domain's types instead.
///
impl<T: NamedType> NamedType for Vec<T> {
    fn type_name() -> String {
        format!("Vec<{}>", T::type_name())
    }
}

impl<K: NamedType, V: NamedType> NamedType for std::collections::HashMap<K, V> {
    fn type_name() -> String {
        format!("HashMap<{}, {}>", K::type_name(), V::type_name())
    }
}
