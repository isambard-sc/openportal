<!--
SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# IPv6 support for IP allowlisting and server binding

Status: **implemented**. `server.rs`'s bind now uses a typed `SocketAddr`
(§4.1); `IpOrRange` (`config.rs`) tries IPv4 then falls back to IPv6 for
both range validation and matching, with an IPv6 `is_full_ipv6_range`
mirroring the existing IPv4 overflow workaround (§4.2). Covered by new
unit tests mirroring every existing IPv4 case (single address, CIDR
range, invalid syntax, full-range `"::/0"`), plus a `SocketAddr`
round-trip test for the server's own IPv6 bind address and a dedicated
test confirming the client-dial-out URL path (`create_websocket_url`
through to `IntoClientRequest`) already handles a bracketed IPv6 host
literal correctly end to end - the one thing §5 flagged as "expected to
work" rather than already verified. All pre-existing tests continue to
pass unchanged; no config wire-format change for existing IPv4 setups.
`docs/specifications/agent-configuration.md` and `security-model.md` §4.1
updated, including the dual-stack/`IPV6_V6ONLY` caveat as an explicit,
documented non-goal (§2) rather than something OpenPortal attempts to
solve.

## 1. Goal

OpenPortal operates almost entirely at the WebSocket/TLS layer and above,
which is IP-version-agnostic - most of the codebase genuinely doesn't care
whether a peer is reachable over IPv4 or IPv6. Two places, both in
`paddington`, currently do care and are IPv4-only in practice even though
their types suggest otherwise:

1. **IP-range allowlisting** (`IpOrRange::Range`, `paddington/src/config.rs`) -
   a `[[clients]]` entry's CIDR range is hard-coded to
   `iptools::iprange::IpRange::<IPv4>`, both when validating it at
   config-load time and when matching an incoming connection's address
   against it. A single IP (`IpOrRange::IP(IpAddr)`) already works for
   IPv6 today, since `IpAddr` is family-generic - only the *range* case is
   the gap.
2. **The server's own listen address** (`paddington/src/server.rs`) - built
   as a formatted string (`format!("{}:{}", ip, port)`) and handed to
   `TcpListener::bind`. `Display` for an IPv6 `IpAddr` doesn't add the
   `[...]` brackets the standard socket-address string syntax requires,
   so this would fail for an IPv6 `ServiceConfig.ip` even though the field
   itself is already `IpAddr`, not `Ipv4Addr`.

Goal: make both of these work correctly for IPv6, so an agent can be
configured with an IPv6 listen address and/or IPv6 (single or CIDR)
allowlist entries, with no change to how existing IPv4 configuration is
written or stored.

## 2. Non-goals

- **Dual-stack listening** (one socket accepting both IPv4 and IPv6
  clients). Whether an IPv6 listener also accepts IPv4-mapped connections
  is controlled by the OS-level `IPV6_V6ONLY` socket option, which Rust's
  plain `TcpListener::bind` does not expose and which varies by platform
  default. This is explicitly **out of OpenPortal's control** - an
  operator who wants both families reachable should either rely on their
  OS's dual-stack default or run two listeners (one per family, on
  different ports or interfaces). Documented as a caveat, not solved here.
- **Changing the on-disk/wire representation of `IpOrRange`.** `Range`
  stays a plain `String` - the string's own syntax (colons vs dots)
  already unambiguously determines its address family, so no new field or
  variant tag is needed, and no existing IPv4 config changes shape.
- **IPv6-specific allowlist features** (e.g. scoped/link-local address
  handling, zone IDs like `fe80::1%eth0`). Out of scope - basic IPv6
  unicast addresses and CIDR ranges only, matching the existing IPv4
  feature set exactly.

## 3. Current state, traced through the actual code

- `IpOrRange::new()` and its `Deserialize` impl both construct
  `IpRange::<iptools::iprange::IPv4>` unconditionally for anything that
  isn't a bare `IpAddr` - `config.rs:369` (`Deserialize`), `config.rs:396`
  (`new`). An IPv6 CIDR string fails IPv4 CIDR parsing and is rejected.
- `IpOrRange::matches()` does the same for the runtime check -
  `config.rs:410`.
- `is_full_ipv4_range()` (`config.rs:341`) exists to work around a real
  panic: `iptools::iprange::IPv4::new` computes `len = end - start + 1` as
  a `u32`, which overflows for the true full range (`0.0.0.0/0`, i.e.
  `4294967295 - 0 + 1`). Read directly from the `iptools` source
  (`iprange.rs`), `IPv6::new` has the identical `len = end - start + 1`
  computation as a `u128` - so `"::/0"` (`u128::MAX - 0 + 1`) has the
  exact same overflow risk and needs the exact same kind of workaround.
- `iptools` (already a dependency, `paddington/Cargo.toml`) ships a
  complete, separately-typed `IpRange<IPv6>` with the identical API shape
  as `IpRange<IPv4>` (`new`, `contains`, `get_version`, `len`) - confirmed
  by reading `iprange.rs` directly. No new dependency is needed.
- `server.rs`'s `run_once` binds via
  `TcpListener::bind(format!("{}:{}", config.ip(), config.port()))` - a
  plain string, not a typed `SocketAddr`. `healthcheck.rs`'s listener
  already does this correctly -
  `TcpListener::bind(&SocketAddr::new(ip, port))` - and is the pattern to
  copy.
- The client-dialling-out path (`ServerConfig::get_websocket_url`,
  `create_websocket_url`) goes through the `url` crate, which already
  understands bracketed IPv6 host literals in URLs - this path is
  expected to already work, and will be covered by a test rather than
  taken on faith.

## 4. Chosen approach

### 4.1 Fix the server's own bind address

Change `server.rs`'s `run_once` to build a `SocketAddr` directly
(`SocketAddr::new(config.ip(), config.port())`) instead of formatting a
string, matching `healthcheck.rs`. Mechanical, no behavioural change for
existing IPv4 configs.

### 4.2 Extend `IpOrRange::Range` to support IPv6 CIDR

`Range` stays `Range(String)` - unchanged wire/config shape. Every call
site that currently assumes `IpRange::<IPv4>` tries IPv4 first (preserving
today's behaviour and error messages exactly for anything that already
parses as IPv4), then falls back to IPv6:

- `is_full_ipv4_range()` gains a sibling `is_full_ipv6_range()` (same
  shape: prefix `/0` plus `iptools::ipv6::validate_ip` on the address
  part), for the same overflow-avoidance reason described in §3.
- `Deserialize`/`IpOrRange::new()`: try the existing IPv4 full-range check,
  then `IpRange::<IPv4>::new`; if both fail, try the new IPv6 full-range
  check, then `IpRange::<IPv6>::new`; only error if all four fail (folding
  in both underlying error messages so a genuinely malformed range still
  gets a useful message).
- `IpOrRange::matches()`: same try-IPv4-then-IPv6 shape. No need to first
  inspect `addr`'s own family - constructing the "wrong-family" `IpRange`
  for a given range string simply fails to parse (falls through to the
  next attempt), and `IpRange<T>::contains()` on a mismatched-family
  address is already safe (returns `false` rather than erroring, per the
  `iptools` source).

### 4.3 Documentation

- `agent-configuration.md` §1.1/§1.2: note that `ip` (both the listener
  and `[[clients]]`/`[[servers]]` entries) accepts IPv4 or IPv6, single
  address or CIDR range, with the same syntax either way.
- `security-model.md` §4.1: same note, plus the dual-stack/`IPV6_V6ONLY`
  caveat from §2 above, framed explicitly as outside OpenPortal's control.
- `CHANGELOG.md`: an `Added` entry.

## 5. Testing strategy

- Extend the existing `test_ip_or_range*` tests (`config.rs`) with IPv6
  equivalents of every IPv4 case already covered: single address match/
  no-match, CIDR range match/no-match, invalid syntax rejected, and the
  `"::/0"` full-range case mirroring the existing `"0.0.0.0/0"` one.
- A round-trip test for the server's own bind: constructing a
  `ServiceConfig` with an IPv6 `ip` and confirming
  `SocketAddr::new(config.ip(), config.port())` produces a valid,
  correctly-typed address (a full `TcpListener::bind` isn't exercised in
  unit tests elsewhere in this crate, so this stays consistent with that).
- A quick manual/live check of the client-dial-out path with a
  `wss://[::1]:PORT`-style config, since §3 flags it as "expected to work"
  rather than already verified.

## 6. What this still doesn't cover

- Dual-stack listening (§2) - explicitly out of OpenPortal's control,
  documented as an operator-facing caveat.
- Any change to how `client_ip`/`proxy_header` extraction works
  (`connection.rs`) - already family-agnostic (`std::net::IpAddr`), needs
  no change.
