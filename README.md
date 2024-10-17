# OpenPortal

This is an implementation of the OpenPortal protocol for communication
between a user portal (e.g. Waldur) and digital research infrastructure
(e.g. the Isambard supercomputers provided by BriSC).

## Compiling OpenPortal

OpenPortal is written in Rust, so you will need to have Rust installed.

To compile OpenPortal, run:

```bash
make
```

or

```bash
make release
```

or use the `cargo` command directly:

```bash
cargo build
```

or

```bash
cargo build --release
```

## Installing OpenPortal

The result of compilation will be a number of executable binaries in the
`target/debug` or `target/release` directories. These are static executables
that can be safely copied to their target destinations and run there.

To understand where to install the executables, you will first need to
understand what OpenPortal is, and how it is used. Please see the
[examples](examples) directory for detailed documentation on the
design and implementation of OpenPortal, together with some examples.
