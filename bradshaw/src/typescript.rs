// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! TypeScript bindings (`ts-rs`): every type here is serialised as a string,
//! so it is a `string` in TypeScript - as ts-rs declared chrono's types.

use ts_rs::TS;

use crate::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Utc, Weekday};

macro_rules! typescript_string {
    ($($ty:ty),*) => {$(
        impl TS for $ty {
            type WithoutGenerics = Self;

            fn ident() -> String {
                "string".to_owned()
            }

            fn name() -> String {
                "string".to_owned()
            }

            fn inline() -> String {
                "string".to_owned()
            }

            fn inline_flattened() -> String {
                panic!("{} cannot be flattened", <Self as TS>::name())
            }

            fn decl() -> String {
                panic!("{} cannot be declared", <Self as TS>::name())
            }

            fn decl_concrete() -> String {
                panic!("{} cannot be declared", <Self as TS>::name())
            }
        }
    )*};
}

typescript_string!(
    DateTime<Utc>,
    NaiveDate,
    NaiveDateTime,
    NaiveTime,
    TimeDelta,
    Weekday
);
