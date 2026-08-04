// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::bridge::{notify as bridge_notify, run as bridge_run, status as bridge_status};
use crate::bridgestate::get as get_board;
use crate::command::Command;
use crate::destination::Destinations;
use crate::diagnostics::collect_diagnostics;
use crate::domain::Domain;
use crate::error::Error;
use crate::health::collect_health;
use crate::job::Job;
use crate::notification::Notification;
use crate::notificationstate;
use crate::portal_identifier::PortalIdentifier;

use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Json, Request, State},
    http::header::HeaderMap,
    http::{HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Duration, Utc};
use paddington::config::IpOrRange;
use paddington::{Key, SecretKey};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path,
    sync::Arc,
};
use tokio::{net::TcpListener, sync::Mutex};
use url::Url;
use uuid::Uuid;

type RateLimitMap = HashMap<IpAddr, (u32, DateTime<Utc>)>;
type SharedRateLimitMap = Arc<Mutex<RateLimitMap>>;

/// Header by which a client declares which canonical-string version its
/// `Authorization` signature was computed over.
///
/// Absent means version 1 - the original, ambiguous form - so every existing
/// client keeps working untouched. See [`SignatureVersion`] and
/// `docs/specifications/security-review-2.md` (finding R29).
pub const SIGNATURE_VERSION_HEADER: &str = "X-OpenPortal-Signature-Version";

///
/// Which canonical-string form a signature was computed over.
///
/// **V1** joins the fields with `\n` and nothing else: no length prefixes, no
/// field count, and one of four un-tagged shapes chosen by whether the body is
/// empty and whether a nonce is present. Those shapes are not distinguishable from
/// one another by the signature, so for a POST
///
/// ```text
/// …\n<function>\n<body>\n<nonce>   ==   …\n<function>\n<body ‖ "\n" ‖ nonce>
/// ```
///
/// are the *same bytes* - meaning the *presence* of a nonce is not authenticated,
/// and a request that supplied one cannot be told from a request that folded it
/// into the body. See finding R29.
///
/// **V2** signs a fixed six-field string with every field length-prefixed, so no
/// field's content can be reinterpreted as a field boundary, and begins with a
/// version tag so a V2 string can never collide with a V1 one.
///
/// V1 is still accepted, and is the default when a client sends no
/// [`SIGNATURE_VERSION_HEADER`], because the bridge's clients include portal
/// software this project does not control. It should be refused once every client
/// is known to send V2.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureVersion {
    /// The original form. Ambiguous - see above.
    V1,
    /// Length-prefixed, fixed-arity, version-tagged.
    V2,
}

impl SignatureVersion {
    /// The version a request declares, or `V1` if it declares none.
    ///
    /// An *unrecognised* value is an error rather than a silent fallback to V1:
    /// falling back would let anything downgrade a V2 client to the weaker form by
    /// mangling one header.
    fn from_headers(headers: &HeaderMap) -> Result<Self, AppError> {
        let Some(value) = headers.get(SIGNATURE_VERSION_HEADER) else {
            return Ok(Self::V1);
        };

        match value.to_str() {
            Ok("1") => Ok(Self::V1),
            Ok("2") => Ok(Self::V2),
            other => {
                tracing::error!(
                    "Unrecognised {} header: {:?}",
                    SIGNATURE_VERSION_HEADER,
                    other
                );

                Err(AppError(
                    anyhow::anyhow!("Unrecognised signature version"),
                    Some(StatusCode::BAD_REQUEST),
                ))
            }
        }
    }

    /// The value to send in [`SIGNATURE_VERSION_HEADER`].
    pub fn as_header_value(&self) -> &'static str {
        match self {
            Self::V1 => "1",
            Self::V2 => "2",
        }
    }
}

///
/// Return the OpenPortal authorisation header for the passed datetime,
/// protocol, function, (optional) body bytes, and nonce, signed with the passed
/// key, using the current canonical form ([`SignatureVersion::V2`]).
///
/// The body parameter should be the raw JSON bytes (empty slice for GET requests).
/// This ensures the signature is computed over the exact bytes sent/received,
/// avoiding any serialization fragility.
///
/// A caller using this **must** also send
/// `X-OpenPortal-Signature-Version: 2` ([`SIGNATURE_VERSION_HEADER`]), or the
/// server will verify against the V1 form and reject the request. Use
/// [`sign_api_call_with_version`] to sign the legacy form deliberately.
///
pub fn sign_api_call(
    key: &SecretKey,
    date: &DateTime<Utc>,
    protocol: &str,
    function: &str,
    body: &[u8],
    nonce: Option<&str>,
) -> Result<String, anyhow::Error> {
    sign_api_call_with_version(
        key,
        date,
        protocol,
        function,
        body,
        nonce,
        SignatureVersion::V2,
    )
}

///
/// As [`sign_api_call`], but signing the canonical form of an explicit
/// [`SignatureVersion`]. Needed by the server, which must reproduce whichever form
/// the client declared, and by tests.
///
pub fn sign_api_call_with_version(
    key: &SecretKey,
    date: &DateTime<Utc>,
    protocol: &str,
    function: &str,
    body: &[u8],
    nonce: Option<&str>,
    version: SignatureVersion,
) -> Result<String, anyhow::Error> {
    let date = date.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let body_str =
        std::str::from_utf8(body).with_context(|| "Could not parse body as UTF-8 for signing")?;

    let call_string = match version {
        SignatureVersion::V1 => v1_call_string(protocol, &date, function, body_str, nonce),
        SignatureVersion::V2 => v2_call_string(protocol, &date, function, body_str, nonce),
    };

    let signature = key.expose_secret().sign(call_string)?;
    Ok(format!("OpenPortal {}", signature))
}

/// The original canonical string. Retained verbatim, ambiguity included, so
/// existing clients keep verifying - see [`SignatureVersion`].
fn v1_call_string(
    protocol: &str,
    date: &str,
    function: &str,
    body: &str,
    nonce: Option<&str>,
) -> String {
    if body.is_empty() {
        // GET request - no body
        match nonce {
            Some(n) => format!(
                "{}\napplication/json\n{}\n{}\n{}",
                protocol, date, function, n
            ),
            None => format!("{}\napplication/json\n{}\n{}", protocol, date, function),
        }
    } else {
        // POST request - include raw body bytes
        match nonce {
            Some(n) => format!(
                "{}\napplication/json\n{}\n{}\n{}\n{}",
                protocol, date, function, body, n
            ),
            None => format!(
                "{}\napplication/json\n{}\n{}\n{}",
                protocol, date, function, body
            ),
        }
    }
}

/// The unambiguous canonical string.
///
/// Every field is present exactly once and prefixed with its byte length, so no
/// field's content can be read as a field boundary and the arity is fixed at six
/// regardless of whether the body or nonce is empty. The leading version tag is
/// itself length-prefixed, so a V2 string cannot collide with a V1 one.
///
/// ```text
/// 17:openportal-sig-v2
/// 4:post
/// 16:application/json
/// 29:Mon, 03 Aug 2026 12:00:00 GMT
/// 3:run
/// 42:{"command":"waldur.provider get_offerings"}
/// 19:unique-nonce-abc123
/// ```
///
/// An absent nonce is the empty string (`0:`), which is distinct from a nonce that
/// is present and empty only in that both are rejected upstream - the point is that
/// `0:` cannot be confused with any other field's content.
fn v2_call_string(
    protocol: &str,
    date: &str,
    function: &str,
    body: &str,
    nonce: Option<&str>,
) -> String {
    let field = |value: &str| format!("{}:{}", value.len(), value);

    [
        field("openportal-sig-v2"),
        field(protocol),
        field("application/json"),
        field(date),
        field(function),
        field(body),
        field(nonce.unwrap_or("")),
    ]
    .join("\n")
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub url: Url,
    pub ip: IpAddr,
    pub port: u16,
    pub key: SecretKey,
    pub signal_url: Option<Url>,
    pub notification_url: Option<Url>,
    /// IP address(es)/range(s) of reverse proxies whose forwarded client-IP
    /// headers (`X-Forwarded-For`/`X-Real-IP`) may be trusted. A forwarded
    /// address is only honoured when the actual TCP peer matches one of these
    /// entries; otherwise the real peer address is used. Same comma-separated
    /// IP/CIDR syntax as elsewhere (e.g. `"127.0.0.0/8"` for a Cloudflare
    /// tunnel or in-cluster ingress on loopback). See
    /// docs/specifications/security-review.md (finding F3).
    #[serde(default)]
    pub trusted_proxy: Option<IpOrRange>,
}

fn create_webserver_url(url: &str) -> Result<Url, Error> {
    let url = url
        .parse::<Url>()
        .with_context(|| format!("Could not parse URL: {}", url))?;

    let scheme = match url.scheme() {
        "http" => "http",
        "https" => "https",
        _ => "https",
    };

    let host = url.host_str().unwrap_or("localhost");
    let port = url.port().unwrap_or(match scheme {
        "http" => 80,
        "https" => 443,
        _ => 443,
    });
    let path = url.path();

    // don't add the port if it is the default for the protocol
    match scheme {
        "http" if port == 80 => {
            return Ok(format!("{}://{}{}", scheme, host, path).parse::<Url>()?);
        }
        "https" if port == 443 => {
            return Ok(format!("{}://{}{}", scheme, host, path).parse::<Url>()?);
        }
        _ => {}
    }

    Ok(format!("{}://{}:{}{}", scheme, host, port, path).parse::<Url>()?)
}

fn create_signal_url(signal_url: &str) -> Result<Option<Url>, Error> {
    let url = signal_url
        .parse::<Url>()
        .with_context(|| format!("Could not parse signal URL: {}", signal_url))?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(anyhow::anyhow!("Signal URL must be http or https").into());
    }

    Ok(Some(url))
}

impl Config {
    pub fn new(url: &str, ip: IpAddr, port: u16, signal_url: &str, notification_url: &str) -> Self {
        Self {
            url: create_webserver_url(url).unwrap_or_else(|e| {
                tracing::error!(
                    "Could not parse URL: {} because '{}'. Using http://localhost:{port} instead.",
                    url,
                    e
                );
                #[allow(clippy::unwrap_used)]
                format!("http://localhost:{port}").parse().unwrap()
            }),
            ip,
            port,
            key: Key::generate(),
            signal_url: create_signal_url(signal_url).unwrap_or_else(|e| {
                tracing::error!(
                    "Could not parse signal URL: {} because '{}'. Using None",
                    signal_url,
                    e
                );
                None
            }),
            notification_url: create_signal_url(notification_url).unwrap_or_else(|e| {
                tracing::error!(
                    "Could not parse notification URL: {} because '{}'. Using None",
                    notification_url,
                    e
                );
                None
            }),
            trusted_proxy: None,
        }
    }

    /// Set (or clear, with `None`) the trusted-proxy IP/range allow-list used
    /// to decide whether a forwarded client-IP header may be believed. Uses the
    /// same comma-separated IP/CIDR syntax as an agent's `ip`.
    pub fn set_trusted_proxy(&mut self, value: Option<&str>) -> Result<(), Error> {
        self.trusted_proxy = match value {
            Some(value) => Some(IpOrRange::new(value)?),
            None => None,
        };
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Defaults {
    url: String,
    ip: String,
    port: u16,
    signal_url: String,
    notification_url: String,
}

impl Defaults {
    pub fn parse(
        url: Option<String>,
        ip: Option<String>,
        port: Option<u16>,
        signal_url: Option<String>,
        notification_url: Option<String>,
    ) -> Self {
        Self {
            url: url.unwrap_or("http://localhost:8042".to_owned()),
            ip: ip.unwrap_or("127.0.0.1".to_owned()),
            port: port.unwrap_or(8042),
            signal_url: signal_url.unwrap_or("http://localhost/signal".to_owned()),
            notification_url: notification_url
                .unwrap_or("http://localhost/notification".to_owned()),
        }
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    pub fn ip(&self) -> String {
        self.ip.clone()
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn signal_url(&self) -> String {
        self.signal_url.clone()
    }

    pub fn notification_url(&self) -> String {
        self.notification_url.clone()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Invite {
    pub url: Url,
    pub key: SecretKey,
}

impl Invite {
    pub fn parse(url: &Url, key: &SecretKey) -> Self {
        Self {
            url: url.clone(),
            key: key.clone(),
        }
    }
}

#[allow(dead_code)]
pub fn load(invite_file: &path::PathBuf) -> Result<Invite, Error> {
    // read the invite file
    let invite = std::fs::read_to_string(invite_file)
        .with_context(|| format!("Could not read invite file: {:?}", invite_file))?;

    // parse the invite file
    let invite: Invite = toml::from_str(&invite)
        .with_context(|| format!("Could not parse invite file from toml: {:?}", invite_file))?;

    Ok(invite)
}

/// Write a bridge invite to `invite_file`.
///
/// The invite holds the bridge's HMAC API key in cleartext hex, so it is
/// written owner-only (mode 0600) via `paddington::config::write_secret_file` -
/// the same helper the paddington service config and invites use. It was
/// previously written with a plain `std::fs::write`, landing at the process
/// umask (commonly 0644) and so readable by any local user, who could then sign
/// arbitrary bridge API requests. See
/// docs/specifications/security-review-2.md (finding R9).
pub fn save(invite: &Invite, invite_file: &path::PathBuf) -> Result<(), Error> {
    // serialise to toml
    let invite_toml =
        toml::to_string(invite).with_context(|| "Could not serialise invite to toml")?;

    paddington::config::write_secret_file(invite_file, &invite_toml)
        .with_context(|| format!("Could not write invite file: {:?}", invite_file))?;

    Ok(())
}

///
/// Internal header into which `resolve_client_ip_middleware` writes the
/// authoritative client IP for `extract_client_ip` to read. The middleware
/// always strips any inbound copy first, so a client cannot set it itself.
/// This is not part of the public API and clients must not send it.
const RESOLVED_CLIENT_IP_HEADER: &str = "x-openportal-client-ip";

/// Maximum request body the bridge will buffer.
///
/// axum's default is 2 MiB, and the `Bytes` extractor buffers the whole body
/// *before* the handler runs - so `verify_headers` then computed an HMAC-SHA512
/// over up to 2 MiB and `sign_api_call` formatted a second ~2 MiB `String` copy,
/// all for a request that had not authenticated yet. At the rate limiter's
/// (deliberately generous) 10,000 requests / 10 s per address, that is gigabytes
/// of pre-authentication hashing and copying per source address.
///
/// 1 MiB is well above any legitimate call - the largest are a `send_result`
/// carrying a completed Job - while halving the worst case. It is deliberately not
/// tightened further without measuring real payloads, since a too-small limit
/// would reject legitimate work. See
/// docs/specifications/security-review-2.md (finding R24).
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Maximum number of requests handled concurrently, mirroring paddington's
/// `MAX_UNAUTHENTICATED_CONNECTIONS` (round 1 finding F11), which the bridge never
/// got despite being the externally reachable surface. Excess requests are
/// refused with 503 rather than queued, so the failure is fast and visible rather
/// than an unbounded backlog. See finding R24.
const MAX_CONCURRENT_REQUESTS: usize = 512;

/// Upper bound on distinct source addresses tracked for rate limiting.
///
/// One entry per address, never pruned except probabilistically, meant
/// unauthenticated traffic from many addresses grew this without limit. Well above
/// any real client population for a bridge, which serves one portal application.
/// See finding R33.
const MAX_RATE_LIMIT_ENTRIES: usize = 8192;

/// Deadline for handling one request, measured from the point axum has parsed the
/// headers. This also bounds the slow-body half of a slowloris, because the `Bytes`
/// extractor runs inside the handler and therefore inside this deadline.
///
/// A *pre-header* read timeout would have to be set on hyper's builder, which
/// `axum::serve` does not expose; that half remains uncovered, and is why the
/// bridge must stay on an internal network (see `docs/specifications/bridge-api.md`).
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Permits for [`limit_concurrency_middleware`].
static REQUEST_PERMITS: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(MAX_CONCURRENT_REQUESTS);

///
/// Refuse a request outright when `MAX_CONCURRENT_REQUESTS` are already in flight.
///
/// Fail-fast rather than queue, matching how paddington's listener treats its
/// unauthenticated-connection pool. See finding R24.
///
async fn limit_concurrency_middleware(request: Request, next: Next) -> Response {
    let Ok(_permit) = REQUEST_PERMITS.try_acquire() else {
        tracing::warn!(
            "Refusing a bridge request: {} are already in flight (the cap).",
            MAX_CONCURRENT_REQUESTS
        );

        return AppError(
            anyhow::anyhow!("Too many concurrent requests"),
            Some(StatusCode::SERVICE_UNAVAILABLE),
        )
        .into_response();
    };

    next.run(request).await
}

///
/// Abandon a request that takes longer than [`REQUEST_TIMEOUT`].
///
/// hyper 1.x adds no default timeout of any kind, so without this a request could
/// occupy a connection - and, with the middleware above, a permit - indefinitely.
///
async fn timeout_middleware(request: Request, next: Next) -> Response {
    match tokio::time::timeout(REQUEST_TIMEOUT, next.run(request)).await {
        Ok(response) => response,
        Err(_) => {
            tracing::warn!(
                "Abandoning a bridge request that exceeded {:?}.",
                REQUEST_TIMEOUT
            );

            AppError(
                anyhow::anyhow!("Request timed out"),
                Some(StatusCode::REQUEST_TIMEOUT),
            )
            .into_response()
        }
    }
}

/// How long a nonce is remembered for replay detection.
const NONCE_TTL_SECONDS: i64 = 30;

/// Hard cap on the number of nonces tracked at once. Nonces are only recorded
/// for requests that have already passed signature verification (see
/// `verify_headers`) and expire after `NONCE_TTL_SECONDS`, so this is a
/// defence-in-depth backstop that should never be reached in normal operation.
/// See docs/specifications/security-review.md (finding F11).
const MAX_NONCE_ENTRIES: usize = 100_000;

/// Size at which expired nonces are purged. Below this the store is small enough
/// that scanning it is not worth doing on a request path holding a global lock -
/// correctness comes from the per-entry TTL check, not from the purge. See finding
/// R33.
const NONCE_PURGE_THRESHOLD: usize = 4096;

/// Parse the forwarded client IP from `X-Forwarded-For` or `X-Real-IP`. Only
/// consulted for a request whose TCP peer is a configured trusted proxy - never
/// trusted on its own (finding F3).
///
/// `X-Forwarded-For` is a list that each proxy *appends* to, so it reads
/// `<client-supplied…>, <address the last proxy saw>`. The left-most entry is
/// therefore whatever the client sent, i.e. fully attacker-controlled: reading
/// it (as this function originally did) meant an attacker could rotate a fake
/// address per request and get a fresh rate-limit bucket every time - exactly
/// the attack finding F3 set out to close, still working in the very
/// deployment F3 recommends (an appending nginx/ingress/Cloudflare tunnel on
/// loopback).
///
/// So walk the list from the **right**, skipping entries that are themselves
/// trusted proxies, and take the first untrusted one - the "rightmost
/// untrusted" rule. That address was observed by a proxy we trust, rather than
/// asserted by the client. See
/// docs/specifications/security-review-2.md (finding R11).
fn forwarded_ip(headers: &HeaderMap, trusted_proxy: Option<&IpOrRange>) -> Option<IpAddr> {
    if let Some(forwarded) = headers.get("X-Forwarded-For") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            let entries: Vec<&str> = forwarded_str.split(',').collect();

            for entry in entries.iter().rev() {
                let Ok(ip) = entry.trim().parse::<IpAddr>() else {
                    // An unparseable entry means we can no longer tell which
                    // hop appended what, so stop rather than skip past it and
                    // risk believing a client-supplied value further left.
                    break;
                };

                let is_trusted_hop = trusted_proxy
                    .map(|trusted| trusted.matches(&ip))
                    .unwrap_or(false);

                if !is_trusted_hop {
                    return Some(ip);
                }
            }
        }
    }

    // `X-Real-IP` is a single value set by the adjacent proxy rather than an
    // appended list, so there is no left/right ambiguity to resolve.
    if let Some(real_ip) = headers.get("X-Real-IP") {
        if let Ok(ip_str) = real_ip.to_str() {
            if let Ok(ip) = ip_str.trim().parse::<IpAddr>() {
                return Some(ip);
            }
        }
    }

    None
}

/// Middleware that determines the real client IP and stamps it into
/// `RESOLVED_CLIENT_IP_HEADER`.
///
/// The TCP peer address (`ConnectInfo`) is authoritative unless that peer is a
/// configured trusted proxy, in which case the forwarded client IP is honoured.
/// Any client-supplied copy of the header is removed first, so the value
/// `extract_client_ip` later reads cannot be spoofed. This is what lets rate
/// limiting (and any other IP decision) key on a real, non-forgeable address -
/// see docs/specifications/security-review.md (finding F3).
async fn resolve_client_ip_middleware(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer_ip = peer.ip();

    let trusted_proxy = state.config.trusted_proxy.as_ref();

    let peer_is_trusted = trusted_proxy
        .map(|trusted| trusted.matches(&peer_ip))
        .unwrap_or(false);

    let client_ip = if peer_is_trusted {
        forwarded_ip(request.headers(), trusted_proxy).unwrap_or(peer_ip)
    } else {
        peer_ip
    };

    // Never let a client set the resolved-IP header itself.
    request.headers_mut().remove(RESOLVED_CLIENT_IP_HEADER);
    if let Ok(value) = HeaderValue::from_str(&client_ip.to_string()) {
        request
            .headers_mut()
            .insert(RESOLVED_CLIENT_IP_HEADER, value);
    }

    next.run(request).await
}

/// Read the client IP resolved by `resolve_client_ip_middleware`. Never reads
/// `X-Forwarded-For`/`X-Real-IP` directly - those are only consulted, and only
/// when trusted, by the middleware above.
fn extract_client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    // `None` rather than a silent `127.0.0.1` fallback. The header is always stamped
    // by `resolve_client_ip_middleware`, so its absence means the middleware did not
    // run - a wiring error - and defaulting merged every such request into the
    // loopback rate-limit bucket, where it shares a quota with genuine local traffic.
    // Failing the request makes the misconfiguration visible instead. Not a spoofing
    // concern either way (the header cannot be supplied by a client). See
    // docs/specifications/security-review-2.md (finding R33).
    headers
        .get(RESOLVED_CLIENT_IP_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<IpAddr>().ok())
}

///
/// Verify the headers for the request - this checks the API key, rate limiting, and nonce
/// The body parameter should be the raw request body bytes (empty for GET requests)
///
async fn verify_headers(
    state: &AppState,
    headers: &HeaderMap,
    protocol: &str,
    function: &str,
    body: &[u8],
) -> Result<(), AppError> {
    // A non-UTF-8 body cannot be signed, so it can never authenticate - but it used
    // to surface as a 500 from `sign_api_call`'s `?`, while every other pre-auth
    // rejection is a 4xx. That is an unauthenticated behavioural difference, and 400
    // is the correct status for a malformed request. Checked here so the shape of the
    // refusal does not depend on which layer noticed. See finding R33.
    if std::str::from_utf8(body).is_err() {
        tracing::warn!("Rejecting a request whose body is not valid UTF-8");
        return Err(AppError(
            anyhow::anyhow!("Request body must be valid UTF-8"),
            Some(StatusCode::BAD_REQUEST),
        ));
    }

    // Extract client IP for rate limiting
    let Some(client_ip) = extract_client_ip(headers) else {
        tracing::error!(
            "No resolved client address on this request - `resolve_client_ip_middleware` \
             did not run. Refusing rather than attributing it to loopback."
        );

        return Err(AppError(
            anyhow::anyhow!("Could not determine the client address"),
            Some(StatusCode::INTERNAL_SERVER_ERROR),
        ));
    };

    // Check rate limit first (before expensive crypto operations)
    state.rate_limiter.check_rate_limit(client_ip).await?;

    // (rate-limit entries are pruned inside `check_rate_limit` when the table grows
    // past `MAX_RATE_LIMIT_ENTRIES` - deterministic, and inside the lock it already
    // holds. The old probabilistic sweep here claimed 1% but was 1.17%, and ran an
    // O(n) `retain` on the pre-authentication path. See finding R33.)

    let key = match headers.get("Authorization") {
        Some(key) => key,
        None => {
            tracing::error!("No API key in headers");
            return Err(AppError(
                anyhow::anyhow!("No API key in headers"),
                Some(StatusCode::UNAUTHORIZED),
            ));
        }
    }
    .to_str()
    .unwrap_or_default()
    .to_string();

    let date = match headers.get("Date") {
        Some(date) => date,
        None => {
            tracing::error!("No date in headers");
            return Err(AppError(
                anyhow::anyhow!("No date in headers"),
                Some(StatusCode::UNAUTHORIZED),
            ));
        }
    }
    .to_str()
    .map_err(|e| {
        tracing::error!("Could not parse date: {:?}", e);
        AppError(
            anyhow::anyhow!("Could not parse date"),
            Some(StatusCode::UNAUTHORIZED),
        )
    })?;

    // Extract nonce (optional but recommended)
    let nonce = headers
        .get("X-Nonce")
        .and_then(|n| n.to_str().ok())
        .map(|s| s.to_string());

    let date = DateTime::parse_from_rfc2822(date)
        .map_err(|e| {
            tracing::error!("Could not parse date: {:?}", e);
            AppError(
                anyhow::anyhow!("Could not parse date"),
                Some(StatusCode::UNAUTHORIZED),
            )
        })?
        .with_timezone(&Utc);

    // make sure that this date is within the last 5 seconds
    let now = Utc::now();

    if now - date > Duration::seconds(5) || date - now > Duration::seconds(5) {
        tracing::error!("Date is too old or too far in the future");
        return Err(AppError(
            anyhow::anyhow!("Date is outside acceptable time window"),
            Some(StatusCode::UNAUTHORIZED),
        ));
    }

    // Verify the request signature BEFORE touching any replay/nonce state.
    // The nonce store must only ever be read or grown by an authenticated
    // caller, otherwise an unauthenticated flood of distinct nonces could grow
    // it without bound or probe it (finding F11). The nonce is part of the
    // signed material, so verifying the signature also binds the nonce.

    // Reproduce whichever canonical form the client declared. Absent means V1, so
    // existing clients are unaffected; an unrecognised value is rejected rather
    // than silently downgraded to V1. See `SignatureVersion` (finding R29).
    let signature_version = SignatureVersion::from_headers(headers)?;

    if signature_version == SignatureVersion::V1 {
        tracing::debug!(
            "Request for '{}' is signed with the legacy (V1) canonical string, whose \
             field boundaries are ambiguous. Send '{}: 2' once this client supports it.",
            function,
            SIGNATURE_VERSION_HEADER
        );
    }

    // Generate the expected signature from the raw body bytes
    let expected_key = sign_api_call_with_version(
        &state.config.key,
        &date,
        protocol,
        function,
        body,
        nonce.as_deref(),
        signature_version,
    )?;

    // Compare the provided and expected authorization headers in constant time,
    // using orion's vetted `secure_cmp` rather than a hand-rolled loop
    // (finding F15).
    let matches = paddington::constant_time_eq(key.as_bytes(), expected_key.as_bytes());

    if !matches {
        tracing::error!("API key is invalid");
        // Don't log the actual keys in production to prevent leakage
        tracing::debug!("Expected key length: {}", expected_key.len());
        tracing::debug!("Received key length: {}", key.len());
        return Err(AppError(
            anyhow::anyhow!("API key is invalid!"),
            Some(StatusCode::UNAUTHORIZED),
        ));
    }

    // The request is now authenticated. Only now record the nonce for replay
    // prevention, so unauthenticated requests never reach this state (F11).
    if let Some(ref nonce_value) = nonce {
        let mut nonce_store = state.nonce_store.lock().await;

        let ttl = Duration::seconds(NONCE_TTL_SECONDS);

        // Reject a nonce we have seen within the TTL window (replay).
        //
        // Checked *before* any purge: the per-entry TTL comparison here is what makes
        // the answer correct, so purging is purely memory management and need not run
        // on every request. It used to - an O(n) `retain` per request under one global
        // mutex, with the store reaching ~30k entries at the allowed rate, so every
        // request scanned the whole map serialised behind that lock. See
        // docs/specifications/security-review-2.md (finding R33).
        if let Some(last_used) = nonce_store.get(nonce_value) {
            if now - *last_used < ttl {
                tracing::warn!("Replay attack detected: nonce {} already used", nonce_value);
                return Err(AppError(
                    anyhow::anyhow!("Nonce has already been used (replay attack)"),
                    Some(StatusCode::UNAUTHORIZED),
                ));
            }
        }

        // Purge only when the store is big enough for it to be worth doing.
        if nonce_store.len() >= NONCE_PURGE_THRESHOLD {
            let cutoff = now - ttl;
            let before = nonce_store.len();
            nonce_store.retain(|_, timestamp| *timestamp > cutoff);
            tracing::debug!(
                "Purged {} expired nonces ({} remain)",
                before - nonce_store.len(),
                nonce_store.len()
            );
        }

        // If it is still full, evict the oldest entries rather than refusing the
        // request. Returning 503 here failed *every nonced* request while leaving
        // nonce-**less** ones working, which pushed clients towards the unprotected
        // mode - the opposite of what this store exists to encourage (cf. R29).
        // An evicted nonce is at worst replayable within its remaining TTL, which is
        // strictly better than turning the replay defence off wholesale.
        while nonce_store.len() >= MAX_NONCE_ENTRIES && !nonce_store.contains_key(nonce_value) {
            let oldest = nonce_store
                .iter()
                .min_by_key(|(_, timestamp)| **timestamp)
                .map(|(k, _)| k.clone());

            match oldest {
                Some(oldest) => {
                    tracing::warn!(
                        "Nonce store is full ({} entries) - evicting the oldest nonce to \
                         make room. Something is generating nonces far faster than \
                         expected.",
                        nonce_store.len()
                    );
                    nonce_store.remove(&oldest);
                }
                None => break,
            }
        }

        // Store nonce with current timestamp
        nonce_store.insert(nonce_value.clone(), now);
    }

    Ok(())
}

//
// Rate limiter to track request attempts per IP address
//
#[derive(Clone, Debug)]
struct RateLimiter {
    // Map of IP address to (attempt count, window start time)
    attempts: SharedRateLimitMap,
    max_attempts: u32,
    window_seconds: i64,
}

impl RateLimiter {
    fn new(max_attempts: u32, window_seconds: i64) -> Self {
        Self {
            attempts: Arc::new(Mutex::new(HashMap::new())),
            max_attempts,
            window_seconds,
        }
    }

    async fn check_rate_limit(&self, ip: IpAddr) -> Result<(), AppError> {
        let mut attempts = self.attempts.lock().await;
        let now = Utc::now();

        // Prune expired entries whenever the map is larger than a real client
        // population could explain, rather than relying on a probabilistic caller
        // (`rand::random::<u8>() < 3`, which is 1.17% and ran on the pre-auth path).
        // An entry is only useful for `window_seconds`, so anything older is dead
        // weight - and without this the map grew once per distinct source address,
        // unbounded. See docs/specifications/security-review-2.md (finding R33).
        if attempts.len() >= MAX_RATE_LIMIT_ENTRIES {
            let cutoff = now - Duration::seconds(self.window_seconds * 2);
            let before = attempts.len();
            attempts.retain(|_, (_, timestamp)| *timestamp > cutoff);

            if attempts.len() >= MAX_RATE_LIMIT_ENTRIES {
                // Everything is live, so we are genuinely seeing this many distinct
                // addresses. Refuse rather than grow; the alternative is unbounded
                // memory driven entirely by unauthenticated traffic.
                tracing::warn!(
                    "Rate-limit table is full ({} live entries) - refusing new source \
                     addresses until the window rolls over.",
                    attempts.len()
                );

                if !attempts.contains_key(&ip) {
                    return Err(AppError(
                        anyhow::anyhow!("Too many distinct clients"),
                        Some(StatusCode::TOO_MANY_REQUESTS),
                    ));
                }
            } else {
                tracing::debug!(
                    "Pruned {} expired rate-limit entries",
                    before - attempts.len()
                );
            }
        }

        let entry = attempts.entry(ip).or_insert((0, now));

        // Check if we're in a new time window
        if now - entry.1 > Duration::seconds(self.window_seconds) {
            // Reset the window
            entry.0 = 1;
            entry.1 = now;
            Ok(())
        } else if entry.0 >= self.max_attempts {
            tracing::warn!("Rate limit exceeded for IP: {}", ip);
            Err(AppError(
                anyhow::anyhow!("Rate limit exceeded"),
                Some(StatusCode::TOO_MANY_REQUESTS),
            ))
        } else {
            entry.0 += 1;
            Ok(())
        }
    }

    /// Prune entries whose window has long expired. Retained for tests and for any
    /// future periodic caller; the live path prunes inside `check_rate_limit`.
    #[allow(dead_code)]
    async fn cleanup_old_entries(&self) {
        let mut attempts = self.attempts.lock().await;
        let now = Utc::now();
        let cutoff = now - Duration::seconds(self.window_seconds * 2);

        attempts.retain(|_, (_, timestamp)| *timestamp > cutoff);
    }
}

//
// Shared state for the web API - simple key-value store protected
// by a tokio Mutex.
//
#[derive(Clone, Debug)]
struct AppState {
    config: Config,
    rate_limiter: RateLimiter,
    nonce_store: Arc<Mutex<HashMap<String, DateTime<Utc>>>>,
    // data: Arc<Mutex<HashMap<String, String>>>, <- this is how to have shared state
}

//
// Health check endpoint for the web API
//
#[tracing::instrument(skip_all)]
async fn health<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "get", "health", &[]).await?;
    tracing::debug!("Health check - collecting from all agents");

    let self_peer = agent::get_self(None).await;

    let health = match collect_health::<L>(self_peer.name(), vec![]).await {
        Ok(health) => health,
        Err(e) => {
            tracing::error!("Error collecting health: {:?}", e);
            let mut result = HashMap::new();
            result.insert("status".to_string(), json!("error"));
            return Ok(Json(json!(result)));
        }
    };

    let mut result = HashMap::new();

    result.insert("status".to_string(), json!("ok"));
    result.insert("health".to_string(), json!(health));

    Ok(Json(json!(result)))
}

//
// Restart endpoint for the web API
//
#[derive(Serialize, Deserialize, Debug)]
struct RestartRequest {
    restart_type: String,
    destination: String,
}

#[tracing::instrument(skip_all)]
async fn restart<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "post", "restart", &body).await?;

    let payload: RestartRequest = serde_json::from_slice(&body)?;

    tracing::info!(
        "Restart request - type: {}, destination: {}",
        payload.restart_type,
        payload.destination
    );

    // Send the restart command to self with the full destination
    // This reuses the routing logic in the handler, including zone disambiguation
    let restart_cmd = Command::<L>::restart(&payload.restart_type, &payload.destination);
    let self_peer = agent::get_self(None).await;

    match restart_cmd.send_to(&self_peer).await {
        Ok(_) => {
            tracing::info!(
                "Restart command sent to {} successfully",
                payload.destination
            );
        }
        Err(e) => {
            tracing::error!(
                "Error sending restart command to {}: {:?}",
                payload.destination,
                e
            );
            let mut result = HashMap::new();
            result.insert("status".to_string(), json!("error"));
            return Ok(Json(json!(result)));
        }
    }

    // Return success immediately
    let mut result = HashMap::new();
    result.insert("status".to_string(), json!("ok"));
    result.insert(
        "message".to_string(),
        json!("Restart command sent successfully"),
    );

    Ok(Json(json!(result)))
}

//
// Diagnostics endpoint for the web API
//
#[derive(Serialize, Deserialize, Debug)]
struct DiagnosticsRequest {
    destination: String,
}

#[tracing::instrument(skip_all)]
async fn diagnostics<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "post", "diagnostics", &body).await?;

    let payload: DiagnosticsRequest = serde_json::from_slice(&body)?;

    tracing::info!("Diagnostics request - destination: {}", payload.destination);

    // Collect diagnostics from the specified agent
    let report = match collect_diagnostics::<L>(&payload.destination).await {
        Ok(report) => report,
        Err(e) => {
            tracing::error!(
                "Error collecting diagnostics from {}: {:?}",
                payload.destination,
                e
            );
            let mut result = HashMap::new();
            result.insert("status".to_string(), json!("error"));
            return Ok(Json(json!(result)));
        }
    };

    let mut result = HashMap::new();
    result.insert("status".to_string(), json!("ok"));
    result.insert("report".to_string(), json!(report));

    Ok(Json(json!(result)))
}

//
// Struct to represent the requests to the 'run' endpoint
//
#[derive(Deserialize, Debug)]
struct RunRequest {
    command: String,
}

//
// The 'run' endpoint for the web API. This is the main entry point
// to which commands are submitted to OpenPortal. This will return
// a JSON object that represents the Job that has been created.
//
#[tracing::instrument(skip_all)]
async fn run<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Job<L>>, AppError> {
    verify_headers(&state, &headers, "post", "run", &body).await?;

    let payload: RunRequest = serde_json::from_slice(&body)?;

    tracing::debug!("Running command: {}", payload.command);

    match bridge_run::<L>(&payload.command).await {
        Ok(job) => Ok(Json(job)),
        Err(e) => {
            tracing::error!("Error running command: {:?}", e);
            Err(AppError(e.into(), None))
        }
    }
}

//
// The 'notify' endpoint for the web API. Sends a fire-and-forget notification
// into the agent network via the portal. Returns immediately once the notification
// has been handed off — no result or acknowledgement is ever received back.
//
#[tracing::instrument(skip_all)]
async fn notify<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "post", "notify", &body).await?;

    let payload: RunRequest = serde_json::from_slice(&body)?;

    tracing::debug!("Sending notification: {}", payload.command);

    match bridge_notify::<L>(&payload.command).await {
        Ok(()) => Ok(Json(json!({"status": "ok"}))),
        Err(e) => {
            tracing::error!("Error sending notification: {:?}", e);
            Err(AppError(e.into(), None))
        }
    }
}

//
// Struct to represent the requests to the 'run' endpoint
//
#[derive(Deserialize, Debug)]
struct StatusRequest {
    job: Uuid,
}

///
/// The 'status' endpoint for the web API. This will return the status
/// of the requested Job in the OpenPortal system
///
#[tracing::instrument(skip_all)]
async fn status<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Job<L>>, AppError> {
    verify_headers(&state, &headers, "post", "status", &body).await?;

    let payload: StatusRequest = serde_json::from_slice(&body)?;

    tracing::debug!("Status request for job: {:?}", payload);

    match bridge_status::<L>(&payload.job).await {
        Ok(job) => Ok(Json(job)),
        Err(e) => {
            tracing::error!("Error getting status: {:?}", e);
            Err(AppError(e.into(), None))
        }
    }
}

///
/// The 'fetch_jobs' endpoint for the web API. This will return a list
/// of all of the jobs that OpenPortal has sent to us that we need
/// to process
///
#[tracing::instrument(skip_all)]
async fn fetch_jobs<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<Job<L>>>, AppError> {
    verify_headers(&state, &headers, "get", "fetch_jobs", &[]).await?;

    tracing::debug!("Fetching jobs");

    // get the BridgeBoard
    let board = get_board::<L>().await;
    match board {
        Ok(board) => {
            let jobs = board.read().await.unfinished_jobs();
            Ok(Json(jobs))
        }
        Err(e) => {
            tracing::error!("Error getting jobs: {:?}", e);
            Err(AppError(e.into(), None))
        }
    }
}

///
/// The 'fetch_job' endpoint for the web API. This will return a specific
/// job that OpenPortal has sent to us that we need to process.
///
#[tracing::instrument(skip_all)]
async fn fetch_job<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Job<L>>, AppError> {
    verify_headers(&state, &headers, "post", "fetch_job", &body).await?;

    let uid: Uuid = serde_json::from_slice(&body)?;

    tracing::debug!("fetch_job: {:?}", uid);

    // get the BridgeBoard
    let board = get_board::<L>().await;
    match board {
        Ok(board) => {
            let job = board
                .read()
                .await
                .unfinished_jobs()
                .into_iter()
                .find(|j| j.id() == uid);

            match job {
                Some(job) => Ok(Json(job.clone())),
                None => Err(AppError(
                    anyhow::anyhow!("Job not found"),
                    Some(StatusCode::NOT_FOUND),
                )),
            }
        }
        Err(e) => {
            tracing::error!("Error getting jobs: {:?}", e);
            Err(AppError(e.into(), None))
        }
    }
}

///
/// The 'fetch_notification' endpoint for the web API. Returns a pending notification
/// by its UUID. The web portal calls this after receiving a fetch_notification signal
/// from the bridge, then returns 200 OK to confirm receipt.
///
#[tracing::instrument(skip_all)]
async fn fetch_notification<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Notification<L>>, AppError> {
    verify_headers(&state, &headers, "post", "fetch_notification", &body).await?;

    let uid: Uuid = serde_json::from_slice(&body)?;

    tracing::debug!("fetch_notification: {:?}", uid);

    match notificationstate::get::<L>(uid).await? {
        Some(notification) => Ok(Json(notification)),
        None => Err(AppError(
            anyhow::anyhow!("Notification not found: {}", uid),
            Some(StatusCode::NOT_FOUND),
        )),
    }
}

///
/// The 'send_result' endpoint for the web API. This will send the
/// result of a job that we need to process back to the OpenPortal system.
///
#[tracing::instrument(skip_all)]
async fn send_result<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "post", "send_result", &body).await?;

    let job: Job<L> = serde_json::from_slice(&body)?;

    tracing::debug!("Sending result: {:?}", job);

    // get the BridgeBoard
    let board = get_board::<L>().await;

    match board {
        Ok(board) => {
            let mut board = board.write().await;
            board.update(&job);
            Ok(Json(json!({"status": "ok"})))
        }
        Err(e) => {
            tracing::error!("Error getting jobs: {:?}", e);
            Err(AppError(e.into(), None))
        }
    }
}

#[allow(dead_code)]
const PORTAL_WAIT_TIME: u64 = 5; // seconds

#[tracing::instrument(skip_all)]
async fn get_portal(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<PortalIdentifier>, AppError> {
    tracing::debug!("get_portal");
    verify_headers(&state, &headers, "get", "get_portal", &[]).await?;

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => match PortalIdentifier::parse(portal.name()) {
            Ok(portal) => Ok(Json(portal)),
            Err(e) => {
                tracing::error!("Error getting portal: {:?}", e);
                Err(AppError(e.into(), None))
            }
        },
        None => {
            tracing::error!("No portal agent found");
            Err(AppError(
                anyhow::anyhow!("Cannot get portal because there is no portal agent"),
                None,
            ))
        }
    }
}

#[tracing::instrument(skip_all)]
async fn sync_offerings<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Destinations>, AppError> {
    verify_headers(&state, &headers, "post", "sync_offerings", &body).await?;

    let offerings: Destinations = serde_json::from_slice(&body)?;

    tracing::debug!("sync_offerings: {:?}", offerings);

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::<L>::parse(
                &format!(
                    "{}.{} sync_offerings {}",
                    agent::name().await,
                    portal.name(),
                    offerings
                ),
                false,
            )?
            .put(&portal)
            .await?;

            // Wait for the sync_offerings job to complete
            let result = match job.wait().await?.result::<Destinations>() {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Error synchronizing offerings: {:?}", e);
                    return Err(AppError(e.into(), None));
                }
            };

            match result {
                Some(offerings) => {
                    tracing::info!("Synchronized offerings: {:?}", offerings);
                    Ok(Json(offerings))
                }
                None => {
                    tracing::warn!("No offerings synchronized?");
                    Ok(Json(Destinations::default()))
                }
            }
        }
        None => {
            tracing::error!("No portal agent found");
            Err(AppError(
                anyhow::anyhow!("Cannot run the job because there is no portal agent"),
                None,
            ))
        }
    }
}

#[tracing::instrument(skip_all)]
async fn add_offerings<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Destinations>, AppError> {
    verify_headers(&state, &headers, "post", "add_offerings", &body).await?;

    let offerings: Destinations = serde_json::from_slice(&body)?;

    tracing::debug!("add_offerings: {:?}", offerings);

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::<L>::parse(
                &format!(
                    "{}.{} add_offerings {}",
                    agent::name().await,
                    portal.name(),
                    offerings
                ),
                false,
            )?
            .put(&portal)
            .await?;

            // Wait for the add_offerings job to complete
            let result = match job.wait().await?.result::<Destinations>() {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Error adding offerings: {:?}", e);
                    return Err(AppError(e.into(), None));
                }
            };

            match result {
                Some(offerings) => {
                    tracing::info!("Added offerings: {:?}", offerings);
                    Ok(Json(offerings))
                }
                None => {
                    tracing::warn!("No offerings added?");
                    Ok(Json(Destinations::default()))
                }
            }
        }
        None => {
            tracing::error!("No portal agent found");
            Err(AppError(
                anyhow::anyhow!("Cannot run the job because there is no portal agent"),
                None,
            ))
        }
    }
}

///
/// Function to list offerings in the portal
///
#[tracing::instrument(skip_all)]
async fn get_offerings<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Destinations>, AppError> {
    tracing::debug!("get_offerings");
    verify_headers(&state, &headers, "get", "get_offerings", &[]).await?;

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::<L>::parse(
                &format!("{}.{} get_offerings", agent::name().await, portal.name(),),
                false,
            )?
            .put(&portal)
            .await?;

            // Wait for the get_offerings job to complete
            let result = match job.wait().await?.result::<Destinations>() {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Error getting offerings: {:?}", e);
                    return Err(AppError(e.into(), None));
                }
            };

            match result {
                Some(offerings) => {
                    tracing::info!("Offerings: {:?}", offerings);
                    Ok(Json(offerings))
                }
                None => {
                    tracing::warn!("No offerings found?");
                    Ok(Json(Destinations::default()))
                }
            }
        }
        None => {
            tracing::error!("No portal agent found");
            Err(AppError(
                anyhow::anyhow!("Cannot run the job because there is no portal agent"),
                None,
            ))
        }
    }
}

///
/// Remove offerings from the portal
///
#[tracing::instrument(skip_all)]
async fn remove_offerings<L: Domain>(
    headers: HeaderMap,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Json<Destinations>, AppError> {
    verify_headers(&state, &headers, "post", "remove_offerings", &body).await?;

    let offerings: Destinations = serde_json::from_slice(&body)?;

    tracing::debug!("remove_offerings: {:?}", offerings);

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::<L>::parse(
                &format!(
                    "{}.{} remove_offerings {}",
                    agent::name().await,
                    portal.name(),
                    offerings
                ),
                false,
            )?
            .put(&portal)
            .await?;

            // Wait for the remove_offerings job to complete
            let result = match job.wait().await?.result::<Destinations>() {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Error removing offerings: {:?}", e);
                    return Err(AppError(e.into(), None));
                }
            };

            match result {
                Some(offerings) => {
                    tracing::info!("Removed offerings: {:?}", offerings);
                    Ok(Json(offerings))
                }
                None => {
                    tracing::warn!("No offerings removed?");
                    Ok(Json(Destinations::default()))
                }
            }
        }
        None => {
            tracing::error!("No portal agent found");
            Err(AppError(
                anyhow::anyhow!("Cannot run the job because there is no portal agent"),
                None,
            ))
        }
    }
}

///
/// Function spawned to run the API server in a background thread
///
async fn run_server(
    make_service: axum::extract::connect_info::IntoMakeServiceWithConnectInfo<Router, SocketAddr>,
    listener: TcpListener,
) -> Result<()> {
    // `into_make_service_with_connect_info::<SocketAddr>()` is what makes the
    // TCP peer address available to `resolve_client_ip_middleware` via
    // `ConnectInfo` (finding F3).
    match axum::serve(listener, make_service).await {
        Ok(_) => {
            tracing::info!("Server ran successfully");
        }
        Err(e) => {
            tracing::error!("Error starting server: {}", e);
        }
    }

    Ok(())
}

pub async fn spawn<L: Domain>(config: Config) -> Result<(), Error> {
    // create a global state object for the web API
    let state = AppState {
        config: config.clone(),
        rate_limiter: RateLimiter::new(10000, 10), // 10000 requests per 10 seconds
        nonce_store: Arc::new(Mutex::new(HashMap::new())),
        // data: Arc::new(Mutex::new(HashMap::new())),
    };

    // create the web API
    let app = Router::new()
        .route("/", get(|| async { Json(serde_json::Value::Null) }))
        .route("/health", get(health::<L>))
        .route("/restart", post(restart::<L>))
        .route("/diagnostics", post(diagnostics::<L>))
        .route("/run", post(run::<L>))
        .route("/notify", post(notify::<L>))
        .route("/status", post(status::<L>))
        .route("/fetch_job", post(fetch_job::<L>))
        .route("/fetch_jobs", get(fetch_jobs::<L>))
        .route("/fetch_notification", post(fetch_notification::<L>))
        .route("/get_portal", get(get_portal))
        .route("/send_result", post(send_result::<L>))
        .route("/sync_offerings", post(sync_offerings::<L>))
        .route("/add_offerings", post(add_offerings::<L>))
        .route("/get_offerings", get(get_offerings::<L>))
        .route("/remove_offerings", post(remove_offerings::<L>))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            resolve_client_ip_middleware,
        ))
        // Bound what an unauthenticated caller can make us spend. Each `.layer`
        // wraps the ones before it, so the concurrency cap is outermost and
        // rejects before any body is buffered - see the constants above and
        // finding R24.
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(timeout_middleware))
        .layer(middleware::from_fn(limit_concurrency_middleware))
        .with_state(state);

    // create a TCP listener on the specified port
    let listener =
        tokio::net::TcpListener::bind(&std::net::SocketAddr::new(config.ip, config.port)).await?;

    // spawn a new task to run the web server to listen for requests.
    // `into_make_service_with_connect_info` exposes the TCP peer address to the
    // client-IP-resolving middleware (finding F3).
    let make_service = app.into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(run_server(make_service, listener));

    Ok(())
}

// Errors

#[derive(Debug)]
struct AppError(anyhow::Error, Option<axum::http::StatusCode>);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.1.unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        // Log the full error chain server-side, but return only a generic,
        // status-appropriate message to the client - do not echo internal
        // error/Debug detail, which aids reconnaissance (finding F15).
        tracing::error!("Request failed ({}): {:?}", status, self.0);

        let client_message = match status {
            StatusCode::UNAUTHORIZED => "Unauthorized",
            StatusCode::TOO_MANY_REQUESTS => "Too many requests",
            StatusCode::SERVICE_UNAVAILABLE => "Service unavailable",
            StatusCode::BAD_REQUEST => "Bad request",
            StatusCode::NOT_FOUND => "Not found",
            StatusCode::REQUEST_TIMEOUT => "Request timed out",
            StatusCode::PAYLOAD_TOO_LARGE => "Payload too large",
            _ => "Internal server error",
        };

        (status, Json(json!({ "message": client_message }))).into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_client_ip_ignores_forwarded_headers() {
        // extract_client_ip must read ONLY the internal header stamped by the
        // trusted middleware - never a client-supplied X-Forwarded-For /
        // X-Real-IP (finding F3).
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-For", HeaderValue::from_static("1.2.3.4"));
        headers.insert("X-Real-IP", HeaderValue::from_static("5.6.7.8"));

        // No resolved header set -> `None`, so the request is refused rather than
        // attributed to loopback (finding R33). Notably *not* the spoofed IPs.
        assert_eq!(extract_client_ip(&headers), None);

        // With the resolved header set (as the middleware would), that value wins.
        headers.insert(
            RESOLVED_CLIENT_IP_HEADER,
            HeaderValue::from_static("10.0.0.9"),
        );
        assert_eq!(
            extract_client_ip(&headers),
            "10.0.0.9".parse::<IpAddr>().ok()
        );
    }

    #[test]
    fn test_forwarded_ip_takes_rightmost_untrusted_xff_entry() {
        // Regression test for finding R11. This test previously asserted the
        // *first* X-Forwarded-For entry was used, which is the client-supplied
        // one - locking in the very spoof finding F3 set out to close.
        let trusted = IpOrRange::new("10.0.0.0/8")
            .unwrap_or_else(|e| unreachable!("Could not parse trusted range: {:?}", e));

        let mut headers = HeaderMap::new();
        assert_eq!(forwarded_ip(&headers, Some(&trusted)), None);

        headers.insert("X-Real-IP", HeaderValue::from_static("5.6.7.8"));
        assert_eq!(
            forwarded_ip(&headers, Some(&trusted)),
            "5.6.7.8".parse::<IpAddr>().ok()
        );

        // An appending proxy produces "<client-supplied>, <what the proxy
        // saw>". The right-most entry is the one we must believe; the
        // left-most is the attacker's.
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_static("1.2.3.4, 203.0.113.9"),
        );
        assert_eq!(
            forwarded_ip(&headers, Some(&trusted)),
            "203.0.113.9".parse::<IpAddr>().ok(),
            "must take the right-most entry, not the client-supplied left-most one"
        );

        // A chain of trusted hops is skipped over to reach the real client.
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_static("1.2.3.4, 203.0.113.9, 10.1.2.3, 10.4.5.6"),
        );
        assert_eq!(
            forwarded_ip(&headers, Some(&trusted)),
            "203.0.113.9".parse::<IpAddr>().ok()
        );

        // With no trusted_proxy configured, nothing is skipped - the last hop
        // is taken as-is. (The middleware only consults this function at all
        // when the TCP peer is trusted, so this is the single-proxy case.)
        assert_eq!(
            forwarded_ip(&headers, None),
            "10.4.5.6".parse::<IpAddr>().ok()
        );

        // An unparseable entry stops the walk rather than letting us slide
        // left onto a client-supplied value.
        headers.insert(
            "X-Forwarded-For",
            HeaderValue::from_static("1.2.3.4, not-an-ip"),
        );
        assert_eq!(
            forwarded_ip(&headers, Some(&trusted)),
            "5.6.7.8".parse::<IpAddr>().ok(),
            "should fall back to X-Real-IP, never to the left-most XFF entry"
        );
    }

    #[test]
    fn test_forwarded_ip_cannot_be_spoofed_by_prepending_entries() {
        // The concrete F3/R11 attack: the client seeds X-Forwarded-For with a
        // different fake address on every request, hoping for a fresh
        // rate-limit bucket each time. The resolved address must stay put.
        let trusted = IpOrRange::new("127.0.0.0/8")
            .unwrap_or_else(|e| unreachable!("Could not parse trusted range: {:?}", e));

        let real_client = "198.51.100.7"
            .parse::<IpAddr>()
            .unwrap_or_else(|e| unreachable!("Could not parse IP: {:?}", e));

        for spoofed in ["203.0.113.1", "203.0.113.2", "8.8.8.8", "1.1.1.1"] {
            let mut headers = HeaderMap::new();
            let value = format!("{}, {}", spoofed, real_client);
            headers.insert(
                "X-Forwarded-For",
                HeaderValue::from_str(&value)
                    .unwrap_or_else(|e| unreachable!("Could not build header: {:?}", e)),
            );

            assert_eq!(
                forwarded_ip(&headers, Some(&trusted)),
                Some(real_client),
                "spoofed prefix {} must not change the resolved client IP",
                spoofed
            );
        }
    }

    #[test]
    fn test_v2_signing_removes_the_nonce_folding_ambiguity() {
        // The R29 collision: under V1 a POST signed with body B and nonce N produces
        // the *same bytes* as one signed with body `B \n N` and no nonce, because the
        // fields are `\n`-joined with no length prefixes and no field count. So the
        // presence of a nonce was not authenticated.
        let key = Key::generate();
        let date = Utc::now();
        let body = br#"{"command":"waldur.provider get_offerings"}"#;
        let nonce = "unique-nonce-abc123";

        let folded = format!("{}\n{}", String::from_utf8_lossy(body), nonce);

        let sign = |b: &[u8], n: Option<&str>, v| {
            sign_api_call_with_version(&key, &date, "post", "run", b, n, v)
                .unwrap_or_else(|e| unreachable!("sign: {:?}", e))
        };

        // V1: the two are indistinguishable - this is the finding.
        assert_eq!(
            sign(body, Some(nonce), SignatureVersion::V1),
            sign(folded.as_bytes(), None, SignatureVersion::V1),
            "V1 is expected to be ambiguous - if this fails the legacy form changed"
        );

        // V2: they are distinct.
        assert_ne!(
            sign(body, Some(nonce), SignatureVersion::V2),
            sign(folded.as_bytes(), None, SignatureVersion::V2),
            "V2 must distinguish a supplied nonce from one folded into the body"
        );

        // V2 must also not collide with V1 for the same inputs, so a signature can
        // never be replayed across versions.
        assert_ne!(
            sign(body, Some(nonce), SignatureVersion::V1),
            sign(body, Some(nonce), SignatureVersion::V2)
        );

        // And the GET/POST shapes can no longer collide either: under V1 an empty
        // body meant the nonce took the body's slot.
        assert_ne!(
            sign(b"", Some(nonce), SignatureVersion::V2),
            sign(nonce.as_bytes(), None, SignatureVersion::V2)
        );
    }

    #[test]
    fn test_v2_canonical_string_is_length_prefixed_and_fixed_arity() {
        // Seven fields, always, each prefixed with its byte length - so no field's
        // content can be read as a boundary, and an absent nonce still occupies a
        // slot.
        let with_nonce = v2_call_string(
            "post",
            "Mon, 03 Aug 2026 12:00:00 GMT",
            "run",
            "{}",
            Some("abc"),
        );
        let without = v2_call_string("post", "Mon, 03 Aug 2026 12:00:00 GMT", "run", "{}", None);

        assert_eq!(with_nonce.split('\n').count(), 7);
        assert_eq!(without.split('\n').count(), 7);

        assert!(with_nonce.starts_with("17:openportal-sig-v2\n"));
        assert!(with_nonce.ends_with("\n3:abc"));
        assert!(without.ends_with("\n0:"));

        // A field containing the separator cannot shift the boundaries.
        let sneaky = v2_call_string("post", "d", "run", "a\n3:xyz", Some("abc"));
        assert_eq!(sneaky.split('\n').count(), 8); // the body itself contains one
                                                   // "a" + "\n" + "3:xyz" is 7 bytes, so the prefix is 7 - a reader cannot be
                                                   // fooled into treating the embedded "3:xyz" as a field of its own.
        assert!(
            sneaky.contains("7:a\n3:xyz"),
            "body must carry its own length"
        );
    }

    #[test]
    fn test_signature_version_defaults_to_v1_and_rejects_anything_unknown() {
        // Absent means V1, which is what keeps every existing client working.
        let headers = HeaderMap::new();
        assert_eq!(
            SignatureVersion::from_headers(&headers).ok(),
            Some(SignatureVersion::V1)
        );

        for (value, expected) in [("1", SignatureVersion::V1), ("2", SignatureVersion::V2)] {
            let mut headers = HeaderMap::new();
            headers.insert(
                SIGNATURE_VERSION_HEADER,
                HeaderValue::from_static(match value {
                    "1" => "1",
                    _ => "2",
                }),
            );
            assert_eq!(
                SignatureVersion::from_headers(&headers).ok(),
                Some(expected)
            );
        }

        // An unrecognised value must be an error, not a silent fallback to V1 -
        // otherwise mangling one header downgrades a V2 client to the weaker form.
        for bad in ["0", "3", "", "2.0", "two", "v2"] {
            let mut headers = HeaderMap::new();
            headers.insert(
                SIGNATURE_VERSION_HEADER,
                HeaderValue::from_str(bad)
                    .unwrap_or_else(|e| unreachable!("header {:?}: {:?}", bad, e)),
            );
            assert!(
                SignatureVersion::from_headers(&headers).is_err(),
                "{:?} must be rejected rather than treated as V1",
                bad
            );
        }
    }

    #[test]
    fn test_sign_api_call() {
        let key = Key::generate();
        let date = Utc::now();
        let protocol = "get";
        let function = "health";
        let body = b""; // Empty body for GET request
        let nonce = None;

        // explicitly V1: this test pins the *legacy* canonical string, which must
        // keep verifying byte-for-byte so existing clients are unaffected (R29)
        let signed = sign_api_call_with_version(
            &key,
            &date,
            protocol,
            function,
            body,
            nonce,
            SignatureVersion::V1,
        )
        .unwrap_or_default();

        #[allow(clippy::unwrap_used)] // safe to do this in a test
        {
            let expected = format!(
                "OpenPortal {}",
                key.expose_secret()
                    .sign(format!(
                        "{}\napplication/json\n{}\n{}",
                        protocol,
                        date.format("%a, %d %b %Y %H:%M:%S GMT"),
                        function
                    ))
                    .unwrap()
            );

            assert_eq!(signed, expected);
        }
    }

    #[test]
    fn test_sign_api_call_with_body() {
        let key = Key::generate();
        let date = Utc::now();
        let protocol = "post";
        let function = "run";
        let body = b"{\"command\":\"test\"}";
        let nonce = "test-nonce";

        let signed = sign_api_call_with_version(
            &key,
            &date,
            protocol,
            function,
            body,
            Some(nonce),
            SignatureVersion::V1,
        )
        .unwrap_or_default();

        #[allow(clippy::unwrap_used)] // safe to do this in a test
        {
            let expected = format!(
                "OpenPortal {}",
                key.expose_secret()
                    .sign(format!(
                        "{}\napplication/json\n{}\n{}\n{}\n{}",
                        protocol,
                        date.format("%a, %d %b %Y %H:%M:%S GMT"),
                        function,
                        std::str::from_utf8(body).unwrap(),
                        nonce
                    ))
                    .unwrap()
            );

            assert_eq!(signed, expected);
        }
    }
}
