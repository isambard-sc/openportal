// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//! Hand-written `ts-rs` binding for `Job<Hpc>`.
//!
//! `templemeads::job::Job<L>` cannot itself `#[derive(TS)]`: ts-rs's derive
//! requires every generic parameter to also implement `TS` (see
//! `docs/plans/grammar-split-design.md`), which would force `Hpc` - and
//! every other `Domain` - to depend on ts-rs just to be usable as a type
//! parameter.
//!
//! It also turns out `impl TS for Job<Hpc>` directly is not legal Rust:
//! both `TS` (from ts-rs) and `Job` (from templemeads) are foreign to this
//! crate, and the orphan rules only grant an exception when a local type
//! appears as a parameter of the *trait*, not of the type being implemented
//! for - `Hpc` being local doesn't help here. So `JobBinding` below is a
//! zero-sized marker type, local to this crate, that exists purely to host
//! the impl; it is never constructed and has no relation to `Job<Hpc>`
//! beyond sharing a name and JSON shape.
//!
//! That shape is mirrored from `Job<L>`'s actual field layout (see
//! `templemeads/src/job.rs`) - kept honest by
//! `tests::job_shape_matches_binding`, which serialises a real `Job<Hpc>`
//! and checks its JSON keys against this declaration.

use std::path::Path;
use templemeads::job::Status;
use ts_rs::{TypeVisitor, TS};

/// Marker type used only to host `impl TS` for the `Job<Hpc>` JSON shape -
/// see the module docs above for why this can't be `impl TS for Job<Hpc>`
/// directly. Never constructed.
#[allow(dead_code)]
struct JobBinding;

impl TS for JobBinding {
    type WithoutGenerics = Self;

    fn ident() -> String {
        "Job".to_owned()
    }

    fn name() -> String {
        "Job".to_owned()
    }

    fn decl() -> String {
        format!("type {} = {};", Self::name(), Self::inline())
    }

    fn decl_concrete() -> String {
        Self::decl()
    }

    fn inline() -> String {
        "{ id: string, created: number, changed: number, expires: number, version: number, \
         command: string, state: Status, result: string | null, result_type: string | null, \
         forwarded_for: string | null, }"
            .to_string()
    }

    fn inline_flattened() -> String {
        panic!("{} cannot be flattened", Self::name())
    }

    fn visit_dependencies(v: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        v.visit::<Status>();
    }

    fn output_path() -> Option<&'static Path> {
        Some(Path::new("Job.ts"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hpc;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use templemeads::job::Job;

    #[test]
    fn export_bindings_job() {
        #[allow(clippy::expect_used)]
        JobBinding::export_all().expect("Could not export the Job<Hpc> binding");
    }

    /// Guards against the hand-written `inline()` above silently drifting
    /// from `Job<L>`'s real fields (a field added/removed/renamed in
    /// templemeads::job::Job would not otherwise cause a compile error,
    /// since this impl is hand-written rather than derived).
    #[test]
    fn job_shape_matches_binding() {
        #[allow(clippy::expect_used)]
        let job = Job::<Hpc>::parse("portal.cluster add_user alice.myproject.myportal", false)
            .expect("Could not parse a test job");

        #[allow(clippy::expect_used)]
        let json = serde_json::to_value(&job).expect("Could not serialise test job");
        let Value::Object(map) = json else {
            panic!("Job did not serialise to a JSON object");
        };

        let actual_keys: BTreeSet<&str> = map.keys().map(String::as_str).collect();
        let expected_keys: BTreeSet<&str> = [
            "id",
            "created",
            "changed",
            "expires",
            "version",
            "command",
            "state",
            "result",
            "result_type",
            "forwarded_for",
        ]
        .into_iter()
        .collect();

        assert_eq!(
            actual_keys, expected_keys,
            "Job<Hpc>'s JSON keys no longer match the hand-written TS binding in \
             job_bindings.rs - update `inline()` (and this test) to match"
        );
    }
}
