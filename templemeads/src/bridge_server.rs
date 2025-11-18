// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use crate::agent;
use crate::bridge::{run as bridge_run, status as bridge_status};
use crate::bridgestate::get as get_board;
use crate::command::HealthInfo;
use crate::destination::Destinations;
use crate::error::Error;
use crate::grammar::PortalIdentifier;
use crate::job::Job;

use anyhow::{Context, Result};
use axum::{
    extract::{Json, State},
    http::header::HeaderMap,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use chrono::{DateTime, Duration, Utc};
use paddington::{Key, SecretKey};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashMap, net::IpAddr, path, sync::Arc};
use tokio::{net::TcpListener, sync::Mutex};
use url::Url;
use uuid::Uuid;

type RateLimitMap = HashMap<IpAddr, (u32, DateTime<Utc>)>;
type SharedRateLimitMap = Arc<Mutex<RateLimitMap>>;

///
/// Return the OpenPortal authorisation header for the passed datetime,
/// protocol, function, (optional) arguments, and nonce, signed with the passed
/// key.
///
pub fn sign_api_call(
    key: &SecretKey,
    date: &DateTime<Utc>,
    protocol: &str,
    function: &str,
    arguments: &Option<serde_json::Value>,
    nonce: Option<&str>,
) -> Result<String, anyhow::Error> {
    let date = date.format("%a, %d %b %Y %H:%M:%S GMT").to_string();

    let call_string = match (arguments, nonce) {
        (Some(args), Some(n)) => format!(
            "{}\napplication/json\n{}\n{}\n{}\n{}",
            protocol, date, function, args, n
        ),
        (Some(args), None) => format!(
            "{}\napplication/json\n{}\n{}\n{}",
            protocol, date, function, args
        ),
        (None, Some(n)) => format!(
            "{}\napplication/json\n{}\n{}\n{}",
            protocol, date, function, n
        ),
        (None, None) => format!("{}\napplication/json\n{}\n{}", protocol, date, function),
    };

    let signature = key.expose_secret().sign(call_string)?;
    Ok(format!("OpenPortal {}", signature))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    pub url: Url,
    pub ip: IpAddr,
    pub port: u16,
    pub key: SecretKey,
    pub signal_url: Option<Url>,
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
        "http" => {
            if port == 80 {
                return Ok(format!("{}://{}{}", scheme, host, path).parse::<Url>()?);
            }
        }
        "https" => {
            if port == 443 {
                return Ok(format!("{}://{}{}", scheme, host, path).parse::<Url>()?);
            }
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
    pub fn new(url: &str, ip: IpAddr, port: u16, signal_url: &str) -> Self {
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
                #[allow(clippy::unwrap_used)]
                None
            }),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Defaults {
    url: String,
    ip: String,
    port: u16,
    signal_url: String,
}

impl Defaults {
    pub fn parse(
        url: Option<String>,
        ip: Option<String>,
        port: Option<u16>,
        signal_url: Option<String>,
    ) -> Self {
        Self {
            url: url.unwrap_or("http://localhost:8042".to_owned()),
            ip: ip.unwrap_or("127.0.0.1".to_owned()),
            port: port.unwrap_or(8042),
            signal_url: signal_url.unwrap_or("http://localhost/signal".to_owned()),
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

pub fn save(invite: &Invite, invite_file: &path::PathBuf) -> Result<(), Error> {
    // serialise to toml
    let invite_toml =
        toml::to_string(invite).with_context(|| "Could not serialise invite to toml")?;

    let invite_file_string = invite_file.to_string_lossy();

    let prefix = invite_file.parent().with_context(|| {
        format!(
            "Could not get parent directory for invite file: {:?}",
            invite_file_string
        )
    })?;

    std::fs::create_dir_all(prefix).with_context(|| {
        format!(
            "Could not create parent directory for invite file: {:?}",
            invite_file_string
        )
    })?;

    std::fs::write(invite_file, invite_toml)
        .with_context(|| format!("Could not write invite file: {:?}", invite_file))?;

    Ok(())
}

///
/// Extract client IP from headers (X-Forwarded-For or X-Real-IP), with fallback
///
fn extract_client_ip(headers: &HeaderMap) -> IpAddr {
    // Try X-Forwarded-For first
    if let Some(forwarded) = headers.get("X-Forwarded-For") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            if let Some(first_ip) = forwarded_str.split(',').next() {
                if let Ok(ip) = first_ip.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
    }

    // Try X-Real-IP
    if let Some(real_ip) = headers.get("X-Real-IP") {
        if let Ok(ip_str) = real_ip.to_str() {
            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                return ip;
            }
        }
    }

    // Fallback to localhost (not ideal, but safe default)
    "127.0.0.1".parse::<IpAddr>().unwrap_or_else(|_| {
        // This should never fail, but handle it anyway
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
    })
}

///
/// Verify the headers for the request - this checks the API key, rate limiting, and nonce
///
async fn verify_headers(
    state: &AppState,
    headers: &HeaderMap,
    protocol: &str,
    function: &str,
    arguments: Option<serde_json::Value>,
) -> Result<(), AppError> {
    // Extract client IP for rate limiting
    let client_ip = extract_client_ip(headers);

    // Check rate limit first (before expensive crypto operations)
    state.rate_limiter.check_rate_limit(client_ip).await?;

    // randomly clean up old rate limit entries (1% chance)
    if rand::random::<u8>() < 3 {
        state.rate_limiter.cleanup_old_entries().await;
    }

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

    // Check nonce for replay attack prevention
    if let Some(ref nonce_value) = nonce {
        let mut nonce_store = state.nonce_store.lock().await;

        // Check if nonce has been used before
        if let Some(last_used) = nonce_store.get(nonce_value) {
            // If nonce was used recently, reject as replay attack
            if now - *last_used < Duration::seconds(30) {
                tracing::warn!("Replay attack detected: nonce {} already used", nonce_value);
                return Err(AppError(
                    anyhow::anyhow!("Nonce has already been used (replay attack)"),
                    Some(StatusCode::UNAUTHORIZED),
                ));
            }
        }

        // Store nonce with current timestamp
        nonce_store.insert(nonce_value.clone(), now);

        // Clean up old nonces (older than 30 seconds)
        let cutoff = now - Duration::seconds(30);
        nonce_store.retain(|_, timestamp| *timestamp > cutoff);
    }

    // now generate the expected key
    let expected_key = sign_api_call(
        &state.config.key,
        &date,
        protocol,
        function,
        &arguments,
        nonce.as_deref(),
    )?;

    // Use constant-time comparison to prevent timing attacks
    let key_bytes = key.as_bytes();
    let expected_bytes = expected_key.as_bytes();

    // Constant-time comparison: always compare all bytes
    let mut matches = key_bytes.len() == expected_bytes.len();
    let compare_len = key_bytes.len().min(expected_bytes.len());

    for i in 0..compare_len {
        matches &= key_bytes[i] == expected_bytes[i];
    }

    // If lengths differ, still compare something to maintain constant time
    if key_bytes.len() != expected_bytes.len() {
        for i in compare_len..key_bytes.len().max(expected_bytes.len()) {
            let _ = i; // Ensure compiler doesn't optimize this away
        }
    }

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

    // Periodic cleanup of old entries (optional, can be called periodically)
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
async fn health(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    verify_headers(&state, &headers, "get", "health", None).await?;
    tracing::debug!("Health check - collecting from all agents");

    // Get health from this agent (bridge)
    let agent_name = crate::agent::name().await;
    let agent_type = crate::agent::my_agent_type().await;
    let start_time = crate::agent::start_time().await;
    let engine = crate::agent::engine().await;
    let version = crate::agent::version().await;

    let mut my_health =
        HealthInfo::new(&agent_name, agent_type, true, start_time, &engine, &version);

    let (active, pending, running, completed, duplicates) =
        crate::state::aggregate_job_stats().await;

    my_health.active_jobs = active;
    my_health.pending_jobs = pending;
    my_health.running_jobs = running;
    my_health.completed_jobs = completed;
    my_health.duplicate_jobs = duplicates;

    // Get all connected peers
    let peers = crate::agent::all_peers().await;

    tracing::debug!("Sending health checks to {} peers", peers.len());

    // Send health check to each peer and collect responses
    // We'll do this concurrently with a timeout
    let health_checks: Vec<_> = peers
        .iter()
        .map(|peer| {
            let peer = peer.clone();
            async move {
                let health_check = crate::command::Command::health_check();
                match health_check.send_to(&peer).await {
                    Ok(_) => {
                        tracing::debug!("Sent health check to {}", peer);
                        Some(peer.name().to_string())
                    }
                    Err(e) => {
                        tracing::warn!("Failed to send health check to {}: {}", peer, e);
                        None
                    }
                }
            }
        })
        .collect();

    // Send all health checks concurrently
    for health_check in health_checks {
        tokio::spawn(health_check);
    }

    // Wait 250ms for health responses to come back
    tokio::time::sleep(tokio::time::Duration::from_millis(250)).await;

    // Get all cached health responses
    let cached_health = crate::handler::get_cached_health().await;

    // Add cached health from peers (each has last_updated field)
    for (agent_name, health_info) in cached_health {
        my_health
            .peers
            .insert(agent_name.clone(), health_info.into());
    }

    let mut result = HashMap::new();

    result.insert("status".to_string(), json!("ok"));

    result.insert("health".to_string(), json!(my_health));

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
async fn run(
    headers: HeaderMap,
    State(state): State<AppState>,
    //Query(params): Query<HashMap<String, String>>,
    Json(payload): Json<RunRequest>,
) -> Result<Json<Job>, AppError> {
    verify_headers(
        &state,
        &headers,
        "post",
        "run",
        Some(serde_json::json!({"command": payload.command})),
    )
    .await?;

    tracing::debug!("Running command: {}", payload.command);

    match bridge_run(&payload.command).await {
        Ok(job) => Ok(Json(job)),
        Err(e) => {
            tracing::error!("Error running command: {:?}", e);
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
async fn status(
    headers: HeaderMap,
    State(state): State<AppState>,
    //Query(params): Query<HashMap<String, String>>,
    Json(payload): Json<StatusRequest>,
) -> Result<Json<Job>, AppError> {
    verify_headers(
        &state,
        &headers,
        "post",
        "status",
        Some(serde_json::json!({"job": payload.job})),
    )
    .await?;

    tracing::debug!("Status request for job: {:?}", payload);

    match bridge_status(&payload.job).await {
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
async fn fetch_jobs(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Vec<Job>>, AppError> {
    verify_headers(&state, &headers, "get", "fetch_jobs", None).await?;

    tracing::debug!("Fetching jobs");

    // get the BridgeBoard
    let board = get_board().await;
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
async fn fetch_job(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(uid): Json<Uuid>,
) -> Result<Json<Job>, AppError> {
    verify_headers(
        &state,
        &headers,
        "post",
        "fetch_job",
        Some(serde_json::json!(uid)),
    )
    .await?;

    tracing::debug!("fetch_job: {:?}", uid);

    // get the BridgeBoard
    let board = get_board().await;
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
/// The 'send_result' endpoint for the web API. This will send the
/// result of a job that we need to process back to the OpenPortal system.
///
#[tracing::instrument(skip_all)]
async fn send_result(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(job): Json<Job>,
) -> Result<Json<serde_json::Value>, AppError> {
    tracing::debug!("Send result: {:?}", job);

    verify_headers(
        &state,
        &headers,
        "post",
        "send_result",
        Some(serde_json::json!(job)),
    )
    .await?;

    tracing::debug!("Sending result: {:?}", job);

    // get the BridgeBoard
    let board = get_board().await;

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
    verify_headers(&state, &headers, "get", "get_portal", None).await?;

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
async fn sync_offerings(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(offerings): Json<Destinations>,
) -> Result<Json<Destinations>, AppError> {
    tracing::debug!("sync_offerings: {:?}", offerings);

    verify_headers(
        &state,
        &headers,
        "post",
        "sync_offerings",
        Some(serde_json::json!(offerings)),
    )
    .await?;

    tracing::debug!("sync_offerings: {:?}", offerings);

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::parse(
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
async fn add_offerings(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(offerings): Json<Destinations>,
) -> Result<Json<Destinations>, AppError> {
    tracing::debug!("add_offerings: {:?}", offerings);

    verify_headers(
        &state,
        &headers,
        "post",
        "add_offerings",
        Some(serde_json::json!(offerings)),
    )
    .await?;

    tracing::debug!("add_offerings: {:?}", offerings);

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::parse(
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
async fn get_offerings(
    headers: HeaderMap,
    State(state): State<AppState>,
) -> Result<Json<Destinations>, AppError> {
    tracing::debug!("get_offerings");
    verify_headers(&state, &headers, "get", "get_offerings", None).await?;

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::parse(
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
async fn remove_offerings(
    headers: HeaderMap,
    State(state): State<AppState>,
    Json(offerings): Json<Destinations>,
) -> Result<Json<Destinations>, AppError> {
    tracing::debug!("remove_offerings: {:?}", offerings);

    verify_headers(
        &state,
        &headers,
        "post",
        "remove_offerings",
        Some(serde_json::json!(offerings)),
    )
    .await?;

    tracing::debug!("remove_offerings: {:?}", offerings);

    match agent::portal(PORTAL_WAIT_TIME).await {
        Some(portal) => {
            // send the create_project job to the bridge agent
            let job = Job::parse(
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
async fn run_server(app: Router, listener: TcpListener) -> Result<()> {
    match axum::serve(listener, app).await {
        Ok(_) => {
            tracing::info!("Server ran successfully");
        }
        Err(e) => {
            tracing::error!("Error starting server: {}", e);
        }
    }

    Ok(())
}

pub async fn spawn(config: Config) -> Result<(), Error> {
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
        .route("/health", get(health))
        .route("/run", post(run))
        .route("/status", post(status))
        .route("/fetch_job", post(fetch_job))
        .route("/fetch_jobs", get(fetch_jobs))
        .route("/get_portal", get(get_portal))
        .route("/send_result", post(send_result))
        .route("/sync_offerings", post(sync_offerings))
        .route("/add_offerings", post(add_offerings))
        .route("/get_offerings", get(get_offerings))
        .route("/remove_offerings", post(remove_offerings))
        .with_state(state);

    // create a TCP listener on the specified port
    let listener =
        tokio::net::TcpListener::bind(&std::net::SocketAddr::new(config.ip, config.port)).await?;

    // spawn a new task to run the web server to listen for requests
    tokio::spawn(run_server(app, listener));

    Ok(())
}

// Errors

#[derive(Debug)]
struct AppError(anyhow::Error, Option<axum::http::StatusCode>);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.1.unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"message":format!("Something went wrong: {:?}", self.0)})),
        )
            .into_response()
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
    fn test_sign_api_call() {
        let key = Key::generate();
        let date = Utc::now();
        let protocol = "get";
        let function = "health";
        let arguments = None;
        let nonce = None;

        let signed =
            sign_api_call(&key, &date, protocol, function, &arguments, nonce).unwrap_or_default();

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
}
