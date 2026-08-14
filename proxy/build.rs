// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

fn main() {
    #[allow(clippy::expect_used)]
    built::write_built_file().expect("Failed to acquire build-time information");
}
