// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

//!
//! The Slurm agent's innards, as a library.
//!
//! This exists so that the agent (`src/main.rs`) and the operator tools in
//! `tools/` are one implementation rather than two. A tool that reads usage out
//! of Slurm has to agree with the agent about what a job's usage *is* - how a
//! requeued attempt is classified, how a record is clipped to a window, how a
//! node fraction is billed - and the only way to guarantee that is for both to
//! run the same code.
//!
//! Nothing here is a stable public API. It is published for the binaries in
//! this crate and is expected to change with them.
//!

pub mod cache;
pub mod sacctmgr;
pub mod slurm;

// Used by the agent binary to find its config file, and by nothing in this
// library. Named here so that `unused_crate_dependencies` does not fire on a
// dependency the crate genuinely uses, just not from this target.
use dirs as _;
