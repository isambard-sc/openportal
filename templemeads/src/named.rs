// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

///
/// Trait implemented by any type that can be used as a `Job` result or
/// instruction payload, giving it a stable string name that travels
/// alongside its JSON encoding (see `Job::completed<T>` / `Job::result<T>`).
///
pub trait NamedType {
    fn type_name() -> &'static str;
}

impl NamedType for String {
    fn type_name() -> &'static str {
        "String"
    }
}

impl NamedType for bool {
    fn type_name() -> &'static str {
        "bool"
    }
}

impl NamedType for Vec<String> {
    fn type_name() -> &'static str {
        "Vec<String>"
    }
}
