// SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

use anyhow::Context;
use anyhow::Result;
use chrono::Utc;
use greatwestern::grammar::{ProjectIdentifier, ProjectMapping, UserIdentifier, UserMapping};
use once_cell::sync::Lazy;
use rand::seq::IteratorRandom;
use rand::SeedableRng;
use reqwest::{cookie::CookieStore, cookie::Jar, Client};
use secrecy::{ExposeSecret, SecretString};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use templemeads::job::assert_not_expired;
use templemeads::portal_identifier::PortalIdentifier;
use templemeads::Error;
use tokio::sync::Mutex;

use templemeads::agent::Peer;

use crate::cache;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FreeResponse {
    result: serde_json::Value,
    principal: serde_json::Value,
    error: serde_json::Value,
    id: u16,
}

///
/// Which FreeIPA server should serve a call.
///
/// This matters because a logical operation is usually a read followed by a
/// write that depends on it ("does this user exist? no - then add them"). In
/// a multi-master topology a master that has not yet received a recent add
/// answers "no such user", and adding on the strength of that answer is how
/// the same DN comes to be created twice, leaving 389-ds with a
/// namingConflict it cannot reconcile.
///
#[derive(Debug, Clone)]
enum Target {
    /// Any healthy server in the pool may serve this call. Only safe for
    /// reads whose answer does not decide whether we write.
    Any,
    /// This call must be served by the write server, so that writes - and the
    /// reads that decide whether to write - all meet the same copy of the
    /// directory. Falls back to another server only once the write server is
    /// confirmed down and replication has had time to converge.
    Pinned,
    /// This call must be served by the named server, whatever else the pool
    /// contains.
    Named(String),
}

/// Maximum number of times a single call will re-login and replay after a
/// 401. Without a bound this loop can spin against a server that accepts
/// the login but rejects the session until the job's deadline.
const MAX_LOGIN_RETRIES: u32 = 3;

///
/// Call a post URL on the FreeIPA server described in 'auth'.
///
async fn call_post<T>(
    func: &str,
    args: Option<Vec<String>>,
    kwargs: Option<HashMap<String, String>>,
    expires: &chrono::DateTime<Utc>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    call_post_to(&Target::Any, func, args, kwargs, expires).await
}

///
/// Send a JSON-RPC payload to the passed server, recording whether that server
/// could be reached at all.
///
/// A connection that was refused marks the server down; a call that merely
/// timed out does not. That distinction is the whole point: a slow master is
/// still the master holding our writes, and treating slow as down is how a
/// write ends up on a second master.
///
async fn send_payload(
    client: &Client,
    url: &str,
    server: &str,
    health: &Arc<ServerHealth>,
    payload: &serde_json::Value,
) -> Result<reqwest::Response, SendFailure> {
    match client
        .post(url)
        .header("Referer", format!("{}/ipa", server))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .json(payload)
        .send()
        .await
    {
        Ok(response) => {
            health.mark_up(server);
            Ok(response)
        }
        Err(e) => {
            let message = format!(
                "Could not call function {} on server {}: {}",
                payload, server, e
            );

            if e.is_timeout() {
                health.mark_timeout(server);

                return Err(SendFailure::TimedOut(Error::Timeout(message)));
            }

            if e.is_connect() {
                // nothing is listening - that is not ambiguous
                health.mark_down(server);
            }

            Err(SendFailure::Failed(Error::Call(message)))
        }
    }
}

///
/// Why a call did not get a response, to the extent that the caller has to care
/// about the difference.
///
enum SendFailure {
    /// The server did not answer in time. The call may well have been applied,
    /// so this must never be treated as "the write did not happen".
    TimedOut(Error),
    /// The call did not get a response for some other reason.
    Failed(Error),
}

///
/// Call a post URL that *changes* the directory - an add, a modify, a delete,
/// or a membership change.
///
/// Every one of these goes to the write server. Two masters both accepting an
/// add of the same DN is a conflict 389-ds cannot reconcile, and it cannot be
/// undone with `ipa user-del`, so writes are not something to spread across a
/// multi-master topology for the sake of load.
///
async fn call_write<T>(
    func: &str,
    args: Option<Vec<String>>,
    kwargs: Option<HashMap<String, String>>,
    expires: &chrono::DateTime<Utc>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    call_post_to(&Target::Pinned, func, args, kwargs, expires).await
}

///
/// Call a post URL on a FreeIPA server chosen according to the passed
/// `Target`.
///
async fn call_post_to<T>(
    target: &Target,
    func: &str,
    args: Option<Vec<String>>,
    kwargs: Option<HashMap<String, String>>,
    expires: &chrono::DateTime<Utc>,
) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    // record the start time for this function
    let start_time = Utc::now();

    // get the auth details from the global FreeIPA client
    tracing::debug!(
        "Call post function: {}, args = {:?}, kwargs = {:?}",
        func,
        args,
        kwargs
    );
    tracing::debug!("Getting a connected server...");
    let mut lock = get_connected_server(target, expires).await?;
    tracing::debug!(
        "Connected server obtained! Took {} ms",
        (Utc::now() - start_time).num_milliseconds()
    );

    // how much time is left before we expire?
    let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

    if time_left < 5 {
        return Err(Error::Expired(
            "Not enough time left to call FreeIPA server".to_string(),
        ));
    }

    tracing::debug!(
        "Calling FreeIPA function: {} - we have {} seconds left before we expire",
        func,
        time_left
    );

    // make id a random integer between 1 and 1000
    let id = rand::random::<u16>() % 1000;

    let mut kwargs = kwargs.unwrap_or_default();
    kwargs.insert("version".to_string(), "2.251".to_string());

    // the payload is a json object that contains the method, the parameters
    // (as an array, plus a dict of the version) and a random id. The id
    // will be passed back to us in the response.
    let payload = serde_json::json!({
        "method": func,
        "params": [args.clone().unwrap_or_default(), kwargs.clone()],
        "id": id,
    });

    // no query should take longer than 60 seconds
    // Use a timeout to prevent deadlocks from failed servers
    let client = Client::builder()
        .cookie_provider(Arc::clone(lock.jar()))
        .danger_accept_invalid_certs(should_allow_invalid_certs())
        .timeout(Duration::from_secs(time_left.min(20) as u64))
        .build()
        .context("Could not build client")?;

    // The URL has to be built from the server we are actually about to talk
    // to, and rebuilt after any reconnect below - see the note in the 401
    // loop.
    let mut url = format!("{}/ipa/session/json", lock.server());

    let mut result = match send_payload(&client, &url, lock.server(), lock.health(), &payload).await
    {
        Ok(result) => result,
        Err(SendFailure::TimedOut(e)) => {
            // Throw away the session, so the next call to this server has to
            // log in again. A server that is listening but not answering keeps
            // its session cookie valid for ever, so without this nothing would
            // ever probe it by any other means and it would go on absorbing
            // every write. The login is a separate, short-lived request: if
            // that cannot get through either, the server is confirmed down and
            // failover can begin.
            lock.set_login_failed();
            return Err(e);
        }
        Err(SendFailure::Failed(e)) => return Err(e),
    };

    // write a warning if this took a long time
    if (Utc::now() - start_time).num_seconds() > 5 {
        tracing::warn!(
            "FreeIPA call for server {} to function {} took {} seconds",
            lock.server(),
            func,
            (Utc::now() - start_time).num_seconds()
        );
    } else {
        tracing::debug!(
            "FreeIPA call for server {} to function {} took {} ms",
            lock.server(),
            func,
            (Utc::now() - start_time).num_milliseconds()
        );
    }

    // if this is an authorisation error, try to reconnect
    let mut login_retries: u32 = 0;

    while result.status().as_u16() == 401 {
        tracing::warn!("Login error: 401 - authorisation failed.");
        lock.set_login_failed();

        login_retries = login_retries.saturating_add(1);

        if login_retries > MAX_LOGIN_RETRIES {
            return Err(Error::Login(format!(
                "Could not authorise call to function {} after {} attempts",
                func, MAX_LOGIN_RETRIES
            )));
        }

        // try to get another lock
        drop(lock);

        assert_not_expired(expires)?;

        tracing::error!("Authorisation (401) error. Reconnecting.");
        lock = get_connected_server(target, expires).await?;

        // Reconnecting can hand us a *different* server, so the URL has to be
        // rebuilt. It used to be computed once before this loop, so a replay
        // after reconnecting to another server posted that server's session
        // cookie to the original server, which 401s again - the loop could
        // then only ever end when the job expired.
        url = format!("{}/ipa/session/json", lock.server());

        if Utc::now().signed_duration_since(start_time).num_seconds() > 10 {
            tracing::info!(
                "Call to server {} for function {} has is still running... {} seconds elapsed so far.",
                lock.server(),
                func,
                (Utc::now() - start_time).num_seconds()
            );
        }

        let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

        if time_left < 5 {
            return Err(Error::Expired(
                "Not enough time left to call FreeIPA server".to_string(),
            ));
        }

        let client = Client::builder()
            .cookie_provider(Arc::clone(lock.jar()))
            .danger_accept_invalid_certs(should_allow_invalid_certs())
            .timeout(Duration::from_secs(time_left.min(20) as u64))
            .build()
            .context("Could not build client")?;

        // retry the call
        result = match send_payload(&client, &url, lock.server(), lock.health(), &payload).await {
            Ok(result) => result,
            Err(SendFailure::TimedOut(e)) => {
                lock.set_login_failed();
                return Err(e);
            }
            Err(SendFailure::Failed(e)) => return Err(e),
        };

        if Utc::now().signed_duration_since(start_time).num_seconds() > 10 {
            tracing::error!(
                "Call to server {} for function {} has completed... {} seconds elapsed so far.",
                lock.server(),
                func,
                (Utc::now() - start_time).num_seconds()
            );
        }
    }

    if result.status().is_success() {
        let result = result
            .json::<FreeResponse>()
            .await
            .context("Could not decode from json")?;

        // assert that the id numbers match
        if result.id != id {
            return Err(Error::Call(format!(
                "ID mismatch: expected {}, got {}",
                id, result.id
            )));
        }

        // if there is an error, return it
        if !result.error.is_null() {
            let error_name: &str = result
                .error
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            match error_name {
                "NotFound" => {
                    return Err(Error::NotFound(format!(
                        "Error in response: {:?}",
                        result.error
                    )));
                }
                "DuplicateEntry" => {
                    return Err(Error::Duplicate(format!(
                        "Error in response: {:?}",
                        result.error
                    )));
                }
                _ => {
                    return Err(Error::Call(format!(
                        "Error in response: {:?}",
                        result.error
                    )));
                }
            }
        }

        // return the result, encoded to the type T
        match serde_json::from_value(result.result.clone()).context("Could not decode result") {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::error!("Could not decode result: {:?}. Error: {}", result.result, e);
                tracing::error!("Response: {:?}", result);
                Err(Error::Call(format!(
                    "Could not decode result: {:?}. Error: {}",
                    result.result, e
                )))
            }
        }
    } else {
        tracing::error!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            url,
            result.status(),
            result
        );
        Err(Error::Call(format!(
            "Could not get response for function: {}. Status: {}. Response: {:?}",
            payload,
            result.status(),
            result
        )))
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct IPAResponse {
    count: Option<u32>,
    messages: Option<serde_json::Value>,
    summary: Option<String>,
    result: Option<serde_json::Value>,
    truncated: Option<bool>,
}

impl IPAResponse {
    fn users(&self, project: &ProjectIdentifier) -> Result<Vec<IPAUser>, Error> {
        IPAUser::construct(&self.result.clone().unwrap_or_default(), project)
    }

    fn groups(&self) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct(&self.result.clone().unwrap_or_default())
    }

    fn internal_groups(
        &self,
        internal_groups: &HashMap<String, ProjectIdentifier>,
    ) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct_internal(&self.result.clone().unwrap_or_default(), internal_groups)
    }

    fn legacy_groups(&self, portal: &PortalIdentifier) -> Result<Vec<IPAGroup>, Error> {
        IPAGroup::construct_legacy(&self.result.clone().unwrap_or_default(), portal)
    }
}

#[derive(Debug, Clone)]
struct IPAServer {
    server: String,
    jar: Arc<Jar>,
    user: String,
    password: SecretString,
    num_failed_reconnects: u32,
    last_failed_reconnect: Option<chrono::DateTime<Utc>>,
}

impl IPAServer {
    fn new(server: &str, user: &str, password: &SecretString) -> Self {
        IPAServer {
            server: server.to_string(),
            jar: Arc::new(Jar::default()),
            user: user.to_string(),
            password: password.clone(),
            num_failed_reconnects: 0,
            last_failed_reconnect: None,
        }
    }

    fn is_logged_in(&self) -> bool {
        // check if the jar has a session cookie for this server
        let url = format!("{}/ipa", self.server);

        match url.parse() {
            Ok(url) => {
                let cookies = self.jar.cookies(&url);
                match cookies {
                    Some(cookies) => cookies.to_str().unwrap_or_default().contains("ipa_session"),
                    None => false,
                }
            }
            Err(_) => false,
        }
    }

    fn set_login_failed(&mut self) {
        self.num_failed_reconnects += 1;
        self.last_failed_reconnect = Some(Utc::now());
        self.jar = Arc::new(Jar::default());
    }

    fn set_login_success(&mut self, jar: Arc<Jar>) {
        self.jar = jar;
        self.num_failed_reconnects = 0;
        self.last_failed_reconnect = None;
    }

    fn should_backoff(&self) -> bool {
        if self.num_failed_reconnects < 3 {
            return false;
        }

        if let Some(last_failed) = self.last_failed_reconnect {
            let backoff_duration =
                chrono::Duration::seconds(20 * self.num_failed_reconnects as i64);
            Utc::now() < last_failed + backoff_duration
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct LockedIPAServer {
    server: tokio::sync::OwnedMutexGuard<IPAServer>,
    health: Arc<ServerHealth>,
}

impl LockedIPAServer {
    fn server(&self) -> &str {
        &self.server.server
    }

    fn health(&self) -> &Arc<ServerHealth> {
        &self.health
    }

    fn jar(&self) -> &Arc<Jar> {
        &self.server.jar
    }

    fn set_login_failed(&mut self) {
        self.server.set_login_failed();
    }
}

///
/// One connection slot in the pool. The URL is held outside the mutex so that
/// the pool can be inspected (and a specific server selected) without waiting
/// behind whatever call currently owns that slot.
///
/// The same URL may appear in several slots - that is how the `freeipa-server`
/// option buys concurrency against a single server.
///
#[derive(Debug, Clone)]
struct PoolEntry {
    url: String,
    health: Arc<ServerHealth>,
    server: Arc<Mutex<IPAServer>>,
}

///
/// Whether a server is reachable at all, shared by every slot for that server
/// and held outside their mutexes so it can be read without waiting for an
/// in-flight call.
///
/// Only *confirmed down* is recorded here - a connection that was refused, a
/// name that did not resolve, a login that was rejected. A call that merely
/// timed out does not count: a slow master is still the master that has our
/// writes, and failing over off the back of a timeout is how the same DN ends
/// up added on two of them.
///
#[derive(Debug)]
struct ServerHealth {
    /// Unix seconds at which this server was first confirmed down, or 0 if it
    /// is believed to be up.
    down_since: std::sync::atomic::AtomicI64,
    /// Unix seconds at which this server last came back after being down, or 0
    /// if it has been up for as long as we have known it.
    up_since: std::sync::atomic::AtomicI64,
    /// How many calls in a row this server has failed to answer in time. Reset
    /// by any answer at all.
    consecutive_timeouts: std::sync::atomic::AtomicI64,
}

/// How many calls a server may fail to answer in a row before it is treated as
/// down. A server that is listening but never replies would otherwise take
/// every write forever: nothing marks it down, because a timeout on its own is
/// not evidence that the write did not land.
///
/// Each of these is a separate call that waited out its own timeout, so this is
/// minutes of a server not answering, not one slow request. Failover then still
/// waits the replication window on top.
const MAX_CONSECUTIVE_TIMEOUTS: i64 = 3;

impl ServerHealth {
    fn new() -> Self {
        ServerHealth {
            down_since: std::sync::atomic::AtomicI64::new(0),
            up_since: std::sync::atomic::AtomicI64::new(0),
            consecutive_timeouts: std::sync::atomic::AtomicI64::new(0),
        }
    }

    fn mark_down(&self, url: &str) {
        if self
            .down_since
            .compare_exchange(
                0,
                Utc::now().timestamp(),
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            tracing::warn!("FreeIPA server {} is confirmed down.", url);
        }
    }

    ///
    /// Record that a call to this server did not answer in time, and treat the
    /// server as down once enough of them have gone unanswered in a row.
    ///
    /// A single timeout deliberately does not count. It is indistinguishable
    /// from a write that landed and whose response was lost, and failing over
    /// on the strength of one is what leaves the same DN added on two masters.
    /// A run of them is different: it says the server is not answering
    /// *anything*, and the replication window still has to pass before a write
    /// goes anywhere else - long enough for whatever it did accept to have
    /// replicated to the server that takes over.
    ///
    fn mark_timeout(&self, url: &str) {
        let timeouts = self
            .consecutive_timeouts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .saturating_add(1);

        tracing::warn!(
            "FreeIPA server {} did not answer in time ({} call(s) in a row).",
            url,
            timeouts
        );

        if timeouts >= MAX_CONSECUTIVE_TIMEOUTS {
            tracing::error!(
                "FreeIPA server {} has failed to answer {} calls in a row - \
                 treating it as down.",
                url,
                timeouts
            );
            self.mark_down(url);
        }
    }

    fn mark_up(&self, url: &str) {
        self.consecutive_timeouts
            .store(0, std::sync::atomic::Ordering::SeqCst);

        if self.down_since.swap(0, std::sync::atomic::Ordering::SeqCst) != 0 {
            self.up_since
                .store(Utc::now().timestamp(), std::sync::atomic::Ordering::SeqCst);
            tracing::info!("FreeIPA server {} is reachable again.", url);
        }
    }

    ///
    /// How long this server has been confirmed down, or None if it is
    /// believed to be up.
    ///
    fn down_for(&self) -> Option<chrono::Duration> {
        match self.down_since.load(std::sync::atomic::Ordering::SeqCst) {
            0 => None,
            since => Some(chrono::Duration::seconds(
                Utc::now().timestamp().saturating_sub(since).max(0),
            )),
        }
    }

    ///
    /// Whether this server may be given writes, given how long replication
    /// needs to converge.
    ///
    /// Recovery is as cautious as failover, and for the mirror-image reason: a
    /// server that has just come back has not necessarily caught up with what
    /// another master accepted while it was away, so writing to it too soon
    /// would create the same conflict from the other direction.
    ///
    fn can_take_writes(&self, replication_window: chrono::Duration) -> bool {
        if self.down_since.load(std::sync::atomic::Ordering::SeqCst) != 0 {
            return false;
        }

        match self.up_since.load(std::sync::atomic::Ordering::SeqCst) {
            0 => true,
            since => {
                chrono::Duration::seconds(Utc::now().timestamp().saturating_sub(since).max(0))
                    >= replication_window
            }
        }
    }
}

/// The connection slots that reads may use - one per entry in
/// `freeipa-server`.
static FREEIPA_SERVERS: Lazy<Mutex<Vec<PoolEntry>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// The connection slots reserved for writes, keyed by server.
///
/// Writes get their own slots so that write concurrency can be raised without
/// also multiplying the read pool - and because which server takes the writes
/// changes on failover, so it cannot be expressed by listing one server more
/// often in `freeipa-server`.
///
/// Slots are created the first time a server takes writes and then kept, so
/// that handing the role back to a server that has held it before reuses its
/// sessions instead of logging in again.
static WRITE_SLOTS: Lazy<Mutex<HashMap<String, Vec<PoolEntry>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Whether each server is answering, keyed by server.
///
/// One record per server, shared by every slot for it, read and write alike:
/// evidence that a server has stopped answering is evidence however it was
/// gathered, and failover decisions are made by comparing servers, so they
/// cannot be reading from two separate sets of books.
static SERVER_HEALTH: Lazy<Mutex<HashMap<String, Arc<ServerHealth>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

///
/// The credentials every slot logs in with. Held so that a write slot can be
/// created later, when a server first takes the writes.
///
#[derive(Debug, Clone)]
struct Credentials {
    user: String,
    password: SecretString,
}

static CREDENTIALS: Lazy<Mutex<Option<Credentials>>> = Lazy::new(|| Mutex::new(None));

///
/// The server that all writes are sent to, the time that replication needs to
/// converge before a write may be sent anywhere else, and how many writes may
/// run against that server at once.
///
#[derive(Debug, Clone, Default)]
struct WriteConfig {
    server: Option<String>,
    replication_window: chrono::Duration,
    concurrent_writes: usize,
}

static WRITE_CONFIG: Lazy<Mutex<WriteConfig>> = Lazy::new(|| {
    Mutex::new(WriteConfig {
        server: None,
        replication_window: chrono::Duration::seconds(DEFAULT_REPLICATION_WINDOW),
        concurrent_writes: DEFAULT_CONCURRENT_WRITES,
    })
});

/// How long replication is assumed to need to converge, if
/// `freeipa-replication-window` is not set. A write must not be sent to a
/// different server until at least this long after the write server was last
/// known to be taking writes, or the new server may not yet have heard about
/// what the old one accepted.
const DEFAULT_REPLICATION_WINDOW: i64 = 30;

/// How many writes may run against the write server at once, if
/// `freeipa-concurrent-writes` is not set.
///
/// More than one is safe: a single 389-ds serialises DN uniqueness itself, so
/// two simultaneous adds of one DN on one server give a success and a
/// `DuplicateEntry`, which is handled. It is only two *masters* accepting the
/// same add that cannot be reconciled.
const DEFAULT_CONCURRENT_WRITES: usize = 2;

/// Each concurrent write is a live session on the server, so refuse to open an
/// implausible number of them because of a mistyped config value.
const MAX_CONCURRENT_WRITES: usize = 64;

///
/// Return the health record for the passed server, creating it if this is the
/// first slot to ask.
///
async fn health_for(url: &str) -> Arc<ServerHealth> {
    SERVER_HEALTH
        .lock()
        .await
        .entry(url.to_string())
        .or_insert_with(|| Arc::new(ServerHealth::new()))
        .clone()
}

///
/// Return the connection slots reserved for writes on the passed server,
/// creating them if it has not taken the writes before.
///
async fn write_slots_for(url: &str) -> Result<Vec<PoolEntry>, Error> {
    let mut slots = WRITE_SLOTS.lock().await;

    if let Some(existing) = slots.get(url) {
        return Ok(existing.clone());
    }

    let credentials = match CREDENTIALS.lock().await.clone() {
        Some(credentials) => credentials,
        None => {
            return Err(Error::Call(
                "No FreeIPA credentials have been initialised".to_string(),
            ))
        }
    };

    let concurrent_writes = WRITE_CONFIG.lock().await.concurrent_writes.max(1);
    let health = health_for(url).await;

    let entries: Vec<PoolEntry> = (0..concurrent_writes)
        .map(|_| PoolEntry {
            url: url.to_string(),
            health: health.clone(),
            server: Arc::new(Mutex::new(IPAServer::new(
                url,
                &credentials.user,
                &credentials.password,
            ))),
        })
        .collect();

    tracing::info!(
        "Opened {} write connection(s) to FreeIPA server {}.",
        entries.len(),
        url
    );

    slots.insert(url.to_string(), entries.clone());

    Ok(entries)
}

pub async fn initialise_servers(
    servers: &[String],
    write_server: &str,
    replication_window: Option<i64>,
    concurrent_writes: Option<i64>,
    user: &str,
    password: &SecretString,
) -> Result<(), Error> {
    let mut freeipa_servers = FREEIPA_SERVERS.lock().await;

    // clear any existing servers, along with the write slots and health
    // records that belonged to them
    freeipa_servers.clear();
    WRITE_SLOTS.lock().await.clear();
    SERVER_HEALTH.lock().await.clear();

    *CREDENTIALS.lock().await = Some(Credentials {
        user: user.to_string(),
        password: password.clone(),
    });

    // now add each server
    for server in servers {
        let server = server.trim();

        if server.is_empty() {
            continue;
        }

        freeipa_servers.push(PoolEntry {
            url: server.to_string(),
            health: health_for(server).await,
            server: Arc::new(Mutex::new(IPAServer::new(server, user, password))),
        });
    }

    // Writes all go to one server. In a multi-master topology every master
    // accepts an add, and two masters accepting the same add is a conflict
    // 389-ds cannot reconcile, so spreading writes across them is not load
    // balancing - it is a race. Reads may still go anywhere.
    let write_server = write_server.trim();

    let write_server = match write_server.is_empty() {
        // default to the first configured server
        true => freeipa_servers.first().map(|entry| entry.url.clone()),
        false => {
            if !freeipa_servers
                .iter()
                .any(|entry| entry.url == write_server)
            {
                return Err(Error::Call(format!(
                    "The FreeIPA write server {} is not one of the configured \
                     servers - add it to freeipa-server.",
                    write_server
                )));
            }

            Some(write_server.to_string())
        }
    };

    let replication_window = chrono::Duration::seconds(
        replication_window
            .filter(|window| *window >= 0)
            .unwrap_or(DEFAULT_REPLICATION_WINDOW),
    );

    let concurrent_writes = match concurrent_writes {
        Some(requested) => {
            let clamped = requested.clamp(1, MAX_CONCURRENT_WRITES as i64);

            if clamped != requested {
                tracing::warn!(
                    "freeipa-concurrent-writes of {} is out of range - using {}.",
                    requested,
                    clamped
                );
            }

            clamped as usize
        }
        None => DEFAULT_CONCURRENT_WRITES,
    };

    match &write_server {
        Some(server) => tracing::info!(
            "FreeIPA writes will be sent to {}, up to {} at a time (failing over \
             only once it is confirmed down, and no sooner than {} seconds \
             afterwards).",
            server,
            concurrent_writes,
            replication_window.num_seconds()
        ),
        None => tracing::warn!("No FreeIPA servers configured - writes have nowhere to go."),
    }

    *WRITE_CONFIG.lock().await = WriteConfig {
        server: write_server,
        replication_window,
        concurrent_writes,
    };

    Ok(())
}

///
/// Work out which server a write (or a read that decides a write) should go
/// to.
///
/// Normally that is the configured write server, even if it is busy - the
/// caller waits for a free slot rather than quietly writing somewhere else.
/// It is only allowed elsewhere when the write server has been *confirmed*
/// down (not merely slow) for longer than the replication convergence window,
/// by which point anything it accepted has had time to reach the server we
/// fall back to.
///
async fn resolve_write_target(pool: &[PoolEntry]) -> Target {
    let config = WRITE_CONFIG.lock().await.clone();

    let Some(write_server) = config.server else {
        // no write server known - behave as we did before
        return Target::Any;
    };

    let Some(entry) = pool.iter().find(|entry| entry.url == write_server) else {
        tracing::warn!(
            "The FreeIPA write server {} is no longer in the pool - any server \
             will have to do.",
            write_server
        );
        return Target::Any;
    };

    // the configured write server takes the writes whenever it is in a fit
    // state to, which is the overwhelmingly common case
    if entry.health.can_take_writes(config.replication_window) {
        return Target::Named(write_server);
    }

    // Otherwise elect one replacement, rather than letting writes spread over
    // whatever is left. "One master takes the writes" is the whole point, and
    // it matters most when the topology is already disturbed. Configuration
    // order makes the choice, so every task in this process picks the same
    // server without having to agree about it.
    let replacement = pool
        .iter()
        .find(|candidate| {
            candidate.url != write_server
                && candidate.health.can_take_writes(config.replication_window)
        })
        .map(|candidate| candidate.url.clone());

    match entry.health.down_for() {
        None => {
            // It is up, but came back too recently to be sure it has caught up
            // with what the server that stood in for it accepted. Leave the
            // writes where they are until it has.
            match replacement {
                Some(replacement) => {
                    tracing::warn!(
                        "FreeIPA write server {} is reachable again but came back \
                         less than {} seconds ago, so writes stay on {} until \
                         replication has converged.",
                        write_server,
                        config.replication_window.num_seconds(),
                        replacement
                    );
                    Target::Named(replacement)
                }
                None => Target::Named(write_server),
            }
        }
        Some(down_for) if down_for < config.replication_window => {
            // It is down, but too recently to be sure that another master has
            // caught up with what it already accepted. Keep sending writes at
            // it: failing this call is recoverable, while adding the same DN
            // on a second master is not.
            tracing::warn!(
                "FreeIPA write server {} has been down for {} seconds - not \
                 failing over until {} seconds have passed, so that replication \
                 can converge first.",
                write_server,
                down_for.num_seconds(),
                config.replication_window.num_seconds()
            );
            Target::Named(write_server)
        }
        Some(down_for) => match replacement {
            Some(replacement) => {
                tracing::warn!(
                    "REPLICATION-RISK: FreeIPA write server {} has been down for {} \
                     seconds, so writes will go to {} instead. Anything the old \
                     server accepted but did not replicate before failing may be \
                     added again there, which would leave a replication conflict.",
                    write_server,
                    down_for.num_seconds(),
                    replacement
                );
                Target::Named(replacement)
            }
            None => {
                // Nothing else is in a fit state to take writes either - either
                // everything is down, or the only candidates came back too
                // recently to trust. Keep aiming at the configured write server
                // so the call fails cleanly rather than writing somewhere that
                // may not have caught up.
                tracing::error!(
                    "FreeIPA write server {} has been down for {} seconds and no \
                     other server can safely take writes yet.",
                    write_server,
                    down_for.num_seconds()
                );
                Target::Named(write_server)
            }
        },
    }
}

///
/// Return the distinct FreeIPA servers that this agent is configured to talk
/// to, in configuration order.
///
/// Duplicated entries are collapsed: several slots pointing at the same server
/// are one server as far as replication is concerned.
///
async fn configured_servers() -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();

    for entry in FREEIPA_SERVERS.lock().await.iter() {
        if !urls.contains(&entry.url) {
            urls.push(entry.url.clone());
        }
    }

    urls
}

///
/// The servers to ask when deciding whether something exists, write server
/// first.
///
/// The write server holds our own recent writes, so asking it first answers
/// the common case in one call - and if it is the one that cannot be reached,
/// that is exactly the case the caller needs to warn about.
///
async fn servers_to_check() -> Vec<String> {
    let mut servers = configured_servers().await;

    if let Some(write_server) = WRITE_CONFIG.lock().await.server.clone() {
        // a stable sort on "is not the write server" moves it to the front and
        // leaves the rest in configuration order
        servers.sort_by_key(|url| *url != write_server);
    }

    servers
}

async fn get_connected_server(
    target: &Target,
    expires: &chrono::DateTime<Utc>,
) -> Result<LockedIPAServer, Error> {
    // get a copy of the servers, so that we don't hold the lock while we
    // try to connect
    assert_not_expired(expires)?;

    let pool: Vec<PoolEntry> = FREEIPA_SERVERS.lock().await.clone();

    let freeipa_servers: Vec<PoolEntry> = match target {
        // A write has to be aimed at one specific server, and it takes that
        // server's own write slots rather than competing with reads for the
        // shared ones.
        Target::Pinned => match resolve_write_target(&pool).await {
            Target::Named(url) => write_slots_for(&url).await?,
            // no write server is configured - fall back to how this behaved
            // before there was one
            _ => pool,
        },
        Target::Any => pool,
        // Named is a read of one particular server - the per-master existence
        // checks - so it uses the read pool.
        Target::Named(url) => pool.into_iter().filter(|entry| entry.url == *url).collect(),
    };

    if freeipa_servers.is_empty() {
        return Err(match target {
            Target::Any | Target::Pinned => {
                Error::Call("No FreeIPA servers have been initialised".to_string())
            }
            Target::Named(url) => Error::Call(format!(
                "FreeIPA server {} is not one of the configured servers",
                url
            )),
        });
    }

    let mut rng = rand::rngs::StdRng::from_os_rng();

    loop {
        let mut should_all_backoff: bool = true;

        // randomise the order of the servers for each loop
        for entry in freeipa_servers
            .iter()
            .choose_multiple(&mut rng, freeipa_servers.len())
        {
            assert_not_expired(expires)?;

            match entry.server.clone().try_lock_owned() {
                Ok(mut server) => {
                    if server.is_logged_in() {
                        tracing::debug!("Already logged in to FreeIPA server: {}", server.server);
                        return Ok(LockedIPAServer {
                            server,
                            health: entry.health.clone(),
                        });
                    }

                    if server.should_backoff() {
                        tracing::warn!(
                            "Backing off from trying to login to FreeIPA server: {}",
                            server.server
                        );
                        continue;
                    }

                    should_all_backoff = false;

                    tracing::info!("Logging in to FreeIPA server: {}", server.server);
                    match login(&server.server, &server.user, &server.password, expires).await {
                        Ok(jar) => {
                            // update the jar in the server
                            tracing::info!("Login successful to FreeIPA server: {}", server.server);
                            server.set_login_success(jar);

                            // Deliberately *not* marked up here. Being able to
                            // log in does not prove the server is serving -
                            // one that accepts a login and then hangs on every
                            // call would otherwise have its timeout streak
                            // reset on each re-login and could never be
                            // treated as down. Only an answered call counts.
                            return Ok(LockedIPAServer {
                                server,
                                health: entry.health.clone(),
                            });
                        }
                        Err(e) => {
                            tracing::error!(
                                "Could not login to FreeIPA server: {}. Error: {}",
                                server.server,
                                e
                            );
                            server.set_login_failed();

                            // A server we cannot log in to is confirmed down -
                            // including one that accepted the connection and
                            // then never answered, which is how a hung server
                            // is caught. Running out of the job's own time
                            // budget says nothing about the server, so it does
                            // not count.
                            if !matches!(e, Error::Expired(_)) {
                                entry.health.mark_down(&entry.url);
                            }

                            // release the lock and try the next server
                        }
                    }
                }
                Err(_) => {
                    // could not get the lock - this implies the server is in use
                    should_all_backoff = false;
                }
            }
        }

        if should_all_backoff {
            tracing::error!(
                "All FreeIPA servers ({}) are backing off because of repeated login failures.",
                match target {
                    Target::Any => "any".to_string(),
                    Target::Pinned => "the write server".to_string(),
                    Target::Named(url) => url.clone(),
                }
            );
            return Err(Error::Call(
                "All FreeIPA servers are backing off because of repeated login failures."
                    .to_string(),
            ));
        }

        // wait a bit before trying again
        assert_not_expired(expires)?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn should_allow_invalid_certs() -> bool {
    // One shared implementation of the rule, so a copy here cannot drift into
    // being more permissive than the other agent's.
    templemeads::validate::allow_invalid_ssl_certs()
}

///
/// Login to the FreeIPA server using the passed username and password.
/// This returns a cookie jar that will contain the resulting authorisation
/// cookie, and which can be used for subsequent calls to the server.
///
async fn login(
    server: &str,
    user: &str,
    password: &SecretString,
    expires: &chrono::DateTime<Utc>,
) -> Result<Arc<Jar>, Error> {
    // how much time is left before we expire?
    let time_left = expires.signed_duration_since(Utc::now()).num_seconds();

    if time_left < 5 {
        // Expired, not Call: this is our own job budget running out, not
        // anything wrong with the server, and it must not be read as evidence
        // that the server is down.
        return Err(Error::Expired(
            "Not enough time left to login to FreeIPA server".to_string(),
        ));
    }

    let jar = Arc::new(Jar::default());

    let client = Client::builder()
        .cookie_provider(Arc::clone(&jar))
        .danger_accept_invalid_certs(should_allow_invalid_certs())
        .timeout(Duration::from_secs(time_left.min(10) as u64))
        .build()
        .context("Could not build client")?;

    let url = format!("{}/ipa/session/login_password", server);

    let result = client
        .post(&url)
        .header("Referer", format!("{}/ipa", server))
        .header("Accept", "text/plain")
        // Use `.form()` so the credentials are correctly URL-encoded (it also
        // sets the application/x-www-form-urlencoded Content-Type). Hand-building
        // the body would corrupt auth for a user/password containing `&`, `=`,
        // or `%` (finding F15).
        .form(&[("user", user), ("password", password.expose_secret())])
        .send()
        .await
        .with_context(|| format!("Could not login calling URL: {}", url))?;

    match result.status() {
        status if status.is_success() => Ok(jar),
        _ => Err(Error::Login(format!(
            "Could not login to server: {}. Status: {}. Response: {:?}",
            server,
            result.status(),
            result
        ))),
    }
}

///
/// Public API
///

#[derive(Debug, Clone)]
pub struct IPAUser {
    userid: String,
    cn: UserIdentifier,
    givenname: String,
    homedirectory: String,
    userclass: String,
    primary_group: String,
    memberof: Vec<String>,
    enabled: bool,
}

// implement display for IPAUser
impl std::fmt::Display for IPAUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: local_name={}, givenname={}, userclass={}, primary_group={}, memberof={}, home={}, enabled={}",
            self.identifier(),
            self.userid(),
            self.givenname(),
            self.userclass(),
            self.primary_group(),
            self.memberof().join(","),
            self.home(),
            self.is_enabled()
        )
    }
}

impl IPAUser {
    fn construct(
        result: &serde_json::Value,
        project: &ProjectIdentifier,
    ) -> Result<Vec<IPAUser>, Error> {
        let mut users = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for user in result {
            let userid = user
                .get("uid")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            if userid.is_empty() {
                tracing::error!("Could not find user id: Skipping user.",);
                continue;
            }

            let cn: &str = user
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default();

            let cn = match UserIdentifier::parse(cn) {
                Ok(cn) => match cn.project_identifier() == *project {
                    true => cn,
                    false => {
                        tracing::warn!("Skipping {} as they are not in project {}", cn, project);
                        continue;
                    }
                },
                Err(_) => {
                    // try to guess the user identifier from the username - support legacy
                    match UserIdentifier::parse(&format!(
                        "{}.{}",
                        userid,
                        project.portal_identifier()
                    )) {
                        Ok(cn) => match cn.project_identifier() == *project {
                            true => cn,
                            false => {
                                tracing::warn!(
                                    "Skipping {} as they are not in project {}",
                                    cn,
                                    project
                                );
                                continue;
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "Could not parse user identifier: {}. Error: {}",
                                cn,
                                e
                            );
                            continue;
                        }
                    }
                }
            };

            let givenname = user
                .get("givenname")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let homedirectory: String = user
                .get("homedirectory")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let userclass = user
                .get("userclass")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let memberof: Vec<String> = user
                .get("memberof_group")
                .and_then(|v| v.as_array())
                .map(|v| {
                    v.iter()
                        .filter_map(|v| v.as_str())
                        .map(|v| v.to_string())
                        .collect()
                })
                .unwrap_or_default();

            // try to find the primary group for this user
            let primary_group = get_primary_group(&cn)?.groupid().to_string();

            let primary_group = match memberof.contains(&primary_group) {
                true => primary_group,
                false => {
                    // are they a member of a legacy primary group?
                    let legacy_primary_group = format!("group.{}", project.project());

                    if memberof.contains(&legacy_primary_group) {
                        legacy_primary_group
                    } else {
                        tracing::debug!(
                            "Could not find primary group {} for user: {}",
                            primary_group,
                            cn
                        );
                        "".to_string()
                    }
                }
            };

            // try to see if this user is enabled - the nsaccountlock
            // attribute is changed to "True" when the account is disabled
            let disabled = user
                .get("nsaccountlock")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            users.push(IPAUser {
                userid,
                cn,
                givenname,
                userclass,
                homedirectory,
                primary_group,
                memberof,
                enabled: !disabled,
            });
        }

        Ok(users)
    }

    ///
    /// Return the local user identifier (local unix account)
    /// for this user
    ///
    pub fn userid(&self) -> &str {
        self.local_username()
    }

    ///
    /// Return the givenname for this user (this is the full user.project.portal)
    ///
    pub fn givenname(&self) -> &str {
        &self.givenname
    }

    ///
    /// Return the userclass for this user - it should be "openportal"
    ///
    pub fn userclass(&self) -> &str {
        &self.userclass
    }

    ///
    /// Return the home directory for this user
    ///
    pub fn home(&self) -> &str {
        &self.homedirectory
    }

    ///
    /// Return the primary group for this user - this should be
    /// the project group
    ///
    pub fn primary_group(&self) -> &str {
        &self.primary_group
    }

    ///
    /// Return the groups that this user is a member of
    ///
    pub fn memberof(&self) -> &Vec<String> {
        &self.memberof
    }

    ///
    /// Return the UserIdentifier for this user (user.project.portal)
    ///
    pub fn identifier(&self) -> &UserIdentifier {
        // we have put the OpenPortal UserIdentifier into the
        // "cn" field of the user
        &self.cn
    }

    ///
    /// Return the mapping from the UserIdentifier to the
    /// FreeIPA (local user account plus primary project) user
    ///
    pub fn mapping(&self) -> Result<UserMapping, Error> {
        if self.primary_group.is_empty() {
            // this is a user that doesn't have a primary group - likely because
            // they were disabled. We can guess the primary group, which
            // we will do here, printing out a warning if the user isn't disabled
            let guessed_primary_group = get_primary_group(&self.cn)?.groupid().to_string();

            if self.is_enabled() {
                tracing::warn!(
                    "User {} does not have a primary group. Guessing: {}",
                    self.identifier(),
                    guessed_primary_group
                );
            }

            UserMapping::new(&self.cn, self.userid(), &guessed_primary_group)
        } else {
            UserMapping::new(&self.cn, self.userid(), self.primary_group())
        }
    }

    ///
    /// Return the local username for this user (Unix account)
    ///
    pub fn local_username(&self) -> &str {
        // this is the linux user account
        &self.userid
    }

    ///
    /// Return whether this user is a member of the passed named group
    ///
    pub fn in_group_cn(&self, group_cn: &str) -> bool {
        self.memberof().contains(&group_cn.to_string())
    }

    ///
    /// Return whether this user is in the passed group
    ///
    pub fn in_group(&self, group: &IPAGroup) -> bool {
        self.in_group_cn(group.groupid())
    }

    ///
    /// Return whether or not this user is in all of the passed groups
    ///
    pub fn in_all_groups(&self, groups: &[IPAGroup]) -> bool {
        groups.iter().all(|group| self.in_group(group))
    }

    ///
    /// Return whether or not this user is managed - only users
    /// in the "openportal" group can be managed
    ///
    pub fn is_managed(&self) -> bool {
        let managed_group = match get_managed_group() {
            Ok(group) => group,
            Err(_) => return false,
        };

        match std::env::var("OPENPORTAL_REQUIRE_MANAGED_CLASS") {
            Ok(value) => match value.to_lowercase().as_str() {
                "true" | "yes" | "1" => {
                    self.in_group(&managed_group) && self.userclass() == managed_group.groupid()
                }
                _ => self.in_group(&managed_group),
            },
            Err(_) => self.in_group(&managed_group),
        }
    }

    ///
    /// Return whether or not a user is protected - they are
    /// protected if they are not managed
    ///
    pub fn is_protected(&self) -> bool {
        !self.is_managed()
    }

    ///
    /// Return whether or not this user is enabled in FreeIPA
    ///
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    ///
    /// Return whether or not this user is disabled in FreeIPA
    ///
    pub fn is_disabled(&self) -> bool {
        !self.is_enabled()
    }

    ///
    /// Return whether or not this user is blocked by OpenPortal.
    /// Blocked users are disabled and are members of the "openportal.blocked"
    /// group. This distinguishes them from users that have been removed
    /// (also disabled, but not in the blocked group).
    ///
    pub fn is_blocked(&self) -> bool {
        self.in_group_cn("openportal.blocked")
    }

    ///
    /// Set this user as enabled in FreeIPA
    ///
    pub fn set_enabled(&mut self) {
        self.enabled = true;
    }

    ///
    /// Set this user as disabled in FreeIPA
    ///
    pub fn set_disabled(&mut self) {
        self.enabled = false;
    }
}

#[derive(Debug, Clone)]
pub struct IPAGroup {
    groupid: String,
    identifier: ProjectIdentifier,
    description: String,
}

// implement display for IPAGroup
impl std::fmt::Display for IPAGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: identifier={} description={}",
            self.groupid(),
            self.identifier(),
            self.description()
        )
    }
}

impl IPAGroup {
    fn new(
        groupid: &str,
        identifier: &ProjectIdentifier,
        description: &str,
    ) -> Result<Self, Error> {
        // check that the groupid is valid .... PARSING RULES
        let groupid = groupid.trim();
        let description = description.trim();

        if groupid.is_empty() {
            return Err(Error::Parse("Group identifier is empty".to_string()));
        }

        if description.is_empty() {
            return Err(Error::Parse("Group description is empty".to_string()));
        }

        Ok(IPAGroup {
            groupid: groupid.to_string(),
            identifier: identifier.clone(),
            description: description.to_string(),
        })
    }

    fn construct_internal(
        result: &serde_json::Value,
        internal_groups: &HashMap<String, ProjectIdentifier>,
    ) -> Result<Vec<IPAGroup>, Error> {
        let mut groups = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for group in result {
            // uid is a list of strings - just get the first one
            let groupid = group
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let project = match internal_groups.get(&groupid) {
                Some(project) => project.clone(),
                None => {
                    let managed_group = get_managed_group()?;

                    match groupid == managed_group.groupid() {
                        true => managed_group.identifier().clone(),
                        false => continue,
                    }
                }
            };

            let description = group
                .get("description")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            groups.push(IPAGroup {
                groupid,
                identifier: project,
                description,
            });
        }

        Ok(groups)
    }

    fn construct_legacy(
        result: &serde_json::Value,
        portal: &PortalIdentifier,
    ) -> Result<Vec<IPAGroup>, Error> {
        let mut groups = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for group in result {
            // uid is a list of strings - just get the first one
            let groupid = group
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            // this is a legacy group if the group name is "group.project"
            let parts: Vec<&str> = groupid.split('.').collect();

            // Destructured rather than indexed - the group names come from
            // FreeIPA, not from us. See
            // docs/specifications/security-review-2.md (finding R1).
            let ["group", project_part] = parts.as_slice() else {
                continue;
            };

            let project = match ProjectIdentifier::parse(&format!("{}.{}", project_part, portal)) {
                Ok(project) => project,
                Err(e) => {
                    tracing::warn!("Could not parse project: {}. Error: {}", project_part, e);
                    continue;
                }
            };

            let mut description = group
                .get("description")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            // get the identifier from the description (if possible)
            let identifier = match description.split("|").next() {
                Some(identifier) => match ProjectIdentifier::parse(identifier.trim()) {
                    Ok(identifier) => identifier,
                    Err(_) => {
                        description = format!("{} | {}", project, description);
                        project.clone()
                    }
                },
                None => {
                    description = format!("{} | {}", project, description);
                    project.clone()
                }
            };

            tracing::info!("Constructing legacy group {} / {}", groupid, identifier);

            groups.push(IPAGroup {
                groupid,
                identifier,
                description,
            });
        }

        Ok(groups)
    }

    fn construct(result: &serde_json::Value) -> Result<Vec<IPAGroup>, Error> {
        let mut groups = Vec::new();

        // convert result into an array if it isn't already
        let result = match result.as_array() {
            Some(result) => result.clone(),
            None => vec![result.clone()],
        };

        for group in result {
            // uid is a list of strings - just get the first one
            let groupid = group
                .get("cn")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            let description = group
                .get("description")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();

            // get the identifier from the description (if possible)
            let identifier = match description.split("|").next() {
                Some(identifier) => match ProjectIdentifier::parse(identifier.trim()) {
                    Ok(identifier) => identifier,
                    Err(e) => {
                        tracing::debug!(
                            "Could not parse identifier: {} for {}. Error: {}",
                            identifier,
                            groupid,
                            e
                        );
                        continue;
                    }
                },
                None => {
                    tracing::debug!(
                        "Could not parse identifier from description: {}",
                        description
                    );
                    continue;
                }
            };

            groups.push(IPAGroup {
                groupid,
                identifier,
                description,
            });
        }

        Ok(groups)
    }

    pub fn parse_system_groups(groups: &str) -> Result<Vec<IPAGroup>, Error> {
        let groups = groups.trim();

        if groups.is_empty() {
            return Ok(Vec::new());
        }

        let mut g = Vec::new();
        let mut errors = Vec::new();

        for group in groups.split(',') {
            if !group.is_empty() {
                let project_id = ProjectIdentifier::parse(&format!("{}.system", group))?;

                match IPAGroup::new(group, &project_id, "OpenPortal-managed group") {
                    Ok(group) => g.push(group),
                    Err(e) => {
                        tracing::error!("Could not parse group: {}. Error: {}", group, e);
                        errors.push(group)
                    }
                }
            }
        }

        if !errors.is_empty() {
            return Err(Error::Parse(format!(
                "Could not parse groups: {:?}",
                errors.join(",")
            )));
        }

        Ok(g)
    }

    pub fn parse_instance_groups(groups: &str) -> Result<HashMap<Peer, Vec<IPAGroup>>, Error> {
        let groups = groups.trim();

        if groups.is_empty() {
            return Ok(HashMap::new());
        }

        let mut g: HashMap<Peer, Vec<IPAGroup>> = HashMap::new();
        let mut errors = Vec::new();

        for group in groups.split(',') {
            let parts: Vec<&str> = group.split(':').collect();

            let [instance, group_name] = parts.as_slice() else {
                errors.push(group);
                continue;
            };

            let instance = instance.trim();

            if instance.is_empty() {
                errors.push(group);
                continue;
            }

            let peer = match Peer::parse(instance) {
                Ok(peer) => peer,
                Err(e) => {
                    tracing::error!("Could not parse instance: {}. Error: {}", instance, e);
                    errors.push(group);
                    continue;
                }
            };

            let group = group_name.trim();

            if group.is_empty() {
                errors.push(group);
                continue;
            }

            let project_id = ProjectIdentifier::parse(&format!("{}.instance", group))?;

            match IPAGroup::new(group, &project_id, "OpenPortal-managed group") {
                Ok(group) => {
                    if let Some(groups) = g.get_mut(&peer) {
                        groups.push(group);
                    } else {
                        g.insert(peer.clone(), vec![group]);
                    }
                }
                Err(e) => {
                    tracing::error!("Could not parse group: {}. Error: {}", group, e);
                    errors.push(group)
                }
            }
        }

        if !errors.is_empty() {
            return Err(Error::Parse(format!(
                "Could not parse groups: {:?}",
                errors.join(",")
            )));
        }

        Ok(g)
    }

    pub fn identifier(&self) -> &ProjectIdentifier {
        &self.identifier
    }

    pub fn groupid(&self) -> &str {
        &self.groupid
    }

    pub fn mapping(&self) -> Result<ProjectMapping, Error> {
        ProjectMapping::new(&self.identifier, &self.groupid)
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn is_instance_group(&self) -> bool {
        self.identifier.portal() == "instance"
    }

    pub fn is_project_group(&self) -> bool {
        // Reject every internal portal name, not just `system` and `instance` - this
        // subsumes the former `is_system_group()`, which is why that method is gone.
        // `openportal` was admitted, so `remove_project openportal.openportal` passed
        // this guard and resolved to the *managed* group - `identifier_to_projectid`
        // returns the bare project component for all three internal portals. The blast
        // radius was zero only by accident (an unrelated filter two layers down skips
        // every user, so the group is never emptied or deleted), which is not something
        // a guard should rely on. `get_users`/`force_get_user` already refuse internal
        // portals outright; this brings the predicate into line. See
        // `docs/specifications/security-review-2.md` (finding R33).
        !is_internal_portal(&self.identifier.portal())
    }
}

///
/// Return whether this is an internal reserved portal name, i.e.
/// "openportal", "system", or "instance"
///
fn is_internal_portal(portal: &str) -> bool {
    matches!(portal, "openportal" | "system" | "instance")
}

///
/// Return the Unix project name associated with the passed ProjectIdentifier.
///
/// Eventually we will need to deal with federation, and work
/// out a way to uniquely convert project identifiers to Unix groups.
/// Currently, for the identifier group.portal, we require that
/// a portal ensures group is unique within the portal. For now,
/// we will just use group (as brics is the only portal)
///
/// Note that we also have system groups, which are of the form
/// system.group, and instance groups, which are of the form
/// instance.group. These two names should be reserved and not
/// used for any portals
///
fn identifier_to_projectid(project: &ProjectIdentifier, legacy: bool) -> Result<String, Error> {
    // if the project.portal() is in ["openportal", "system", "instance"]
    // then we just return the project.project()
    let system_portals: Vec<String> = vec![
        "openportal".to_owned(),
        "system".to_owned(),
        "instance".to_owned(),
    ];

    if system_portals.contains(&project.portal()) {
        Ok(project.project().to_string())
    } else if legacy {
        // this is the legacy naming, `group.{project_name}`
        Ok(format!("group.{}", project.project()))
    } else {
        Ok(format!("{}.{}", project.portal(), project.project()))
    }
}

///
/// Return all of the users who are part of the specified group
///
async fn force_get_users_in_group(
    target: &Target,
    group: &IPAGroup,
    expires: &chrono::DateTime<Utc>,
) -> Result<Vec<IPAUser>, Error> {
    if !group.is_project_group() {
        // we only list users in project groups
        tracing::warn!(
            "Not listing users in group {} as it is not a project group",
            group.identifier()
        );
        return Ok(Vec::new());
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("in_group".to_string(), group.groupid().to_string());
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("sizelimit".to_string(), "2048".to_string());
        kwargs
    };

    let result =
        call_post_to::<IPAResponse>(target, "user_find", None, Some(kwargs), expires).await?;

    tracing::debug!(
        "user_find result for group {}: {:?}",
        group.identifier(),
        result
    );

    // filter out users who are not enabled, except for blocked users which are
    // intentionally disabled - we do list unmanaged users, so that OpenPortal
    // isn't repeatedly told to add users who already exist
    Ok(result
        .users(group.identifier())?
        .iter()
        .filter(|u| u.is_enabled() || u.is_blocked())
        .cloned()
        .collect())
}

///
/// Return the specified group from FreeIPA, or None if it does
/// not exist
///
async fn get_group(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAGroup>, Error> {
    match cache::get_group(project).await? {
        Some(group) => Ok(Some(group)),
        None => force_get_group_on(&Target::Any, project, expires).await,
    }
}

///
/// Look the group up in FreeIPA, ignoring the cache, asking the server
/// selected by the passed `Target`.
///
async fn force_get_group_on(
    target: &Target,
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAGroup>, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), identifier_to_projectid(project, false)?);
        kwargs
    };

    tracing::debug!("Call group_find for project: {}", project);
    let result =
        call_post_to::<IPAResponse>(target, "group_find", None, Some(kwargs), expires).await?;
    tracing::debug!("group_find result: {:?}", result);

    if is_internal_portal(&project.portal()) {
        let internal_groups = cache::get_internal_group_ids().await?;

        match result.internal_groups(&internal_groups)?.first() {
            Some(group) => {
                let group = match group.identifier() != project {
                    true => {
                        tracing::warn!(
                            "Disagreement of identifier of found group: {} versus {}",
                            group.identifier(),
                            project
                        );

                        IPAGroup::new(group.groupid(), project, group.description())?
                    }
                    false => group.clone(),
                };

                // add this group to the cache
                cache::add_existing_group(&group).await?;

                Ok(Some(group))
            }
            None => Ok(None),
        }
    } else {
        match result.groups()?.first() {
            Some(group) => {
                let group = match group.identifier() != project {
                    true => {
                        tracing::warn!(
                            "Disagreement of identifier of found group: {} versus {}",
                            group.identifier(),
                            project
                        );

                        IPAGroup::new(group.groupid(), project, group.description())?
                    }
                    false => group.clone(),
                };

                // add this group to the cache - also force get all
                // of the users currently in this group
                cache::add_existing_group(&group).await?;

                // if this is a project group, then get and cache all users
                // in this group
                if group.is_project_group() {
                    let users = force_get_users_in_group(target, &group, expires).await?;
                    cache::set_users_in_group(&group, &users).await?;
                }

                Ok(Some(group))
            }
            None => {
                // try to find the legacy group - this is for porting tech/prep projects
                let kwargs = {
                    let mut kwargs = HashMap::new();
                    kwargs.insert("cn".to_string(), identifier_to_projectid(project, true)?);
                    kwargs
                };

                tracing::debug!("Call group_find for legacy project: {}", project);
                let result =
                    call_post_to::<IPAResponse>(target, "group_find", None, Some(kwargs), expires)
                        .await?;
                tracing::debug!("group_find legacy result: {:?}", result);

                match result.legacy_groups(&project.portal_identifier())?.first() {
                    Some(group) => {
                        let group = match group.identifier() != project {
                            true => {
                                tracing::warn!(
                                    "Disagreement of identifier of found group: {} versus {}",
                                    group.identifier(),
                                    project
                                );

                                IPAGroup::new(group.groupid(), project, group.description())?
                            }
                            false => group.clone(),
                        };

                        // add this group to the cache - also force get all
                        // of the users currently in this group
                        cache::add_existing_group(&group).await?;

                        // if this is a project group, then get and cache all users
                        // in this group
                        if group.is_project_group() {
                            let users = force_get_users_in_group(target, &group, expires).await?;
                            cache::set_users_in_group(&group, &users).await?;
                        }

                        Ok(Some(group))
                    }
                    None => Ok(None),
                }
            }
        }
    }
}

///
/// Return the Unix username associated with the passed UserIdentifier.
///
/// Eventually we will need to deal with federation, and work
/// out a way to uniquely convert user identifiers to Unix usernames.
/// Currently, for the identifier user.group.portal, we require that
/// a portal ensures user.group is unique within the portal. For now,
/// we will just use user.group (as brics is the only portal)
///
pub async fn identifier_to_userid(user: &UserIdentifier) -> Result<String, Error> {
    Ok(format!("{}.{}", user.username(), user.project()))
}

///
/// Force get the user - this will refresh the data from FreeIPA
///
async fn force_get_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAUser>, Error> {
    force_get_user_on(&Target::Any, user, expires).await
}

///
/// Force get the user from the server selected by the passed `Target`.
///
async fn force_get_user_on(
    target: &Target,
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAUser>, Error> {
    // only get users whose portals are not in the internal set
    if is_internal_portal(&user.portal()) {
        return Ok(None);
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("uid".to_string(), identifier_to_userid(user).await?);
        kwargs
    };

    let result =
        call_post_to::<IPAResponse>(target, "user_find", None, Some(kwargs), expires).await?;

    match result.users(&user.project_identifier())?.first() {
        Some(user) => {
            cache::add_existing_user(user).await?;
            Ok(Some(user.clone()))
        }
        None => Ok(None),
    }
}

///
/// Look for the passed user on *every* configured FreeIPA server, returning
/// them from the first server that has them.
///
/// A single `user_find` only says what one master currently knows. A master
/// that has not yet received a recent `user_add` answers "no such user", and
/// "no such user" is the one answer that makes us write - which is how the
/// same DN came to be added twice on two masters and left 389-ds with a
/// namingConflict it cannot reconcile. So before concluding that a user does
/// not exist, ask them all.
///
/// If a server cannot be reached we carry on with the servers that answered,
/// because refusing to provision while one master is down would be worse than
/// the risk of a conflict - but say so loudly, because a conflict seeded here
/// is silent and expensive to clean up.
///
async fn force_get_user_everywhere(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAUser>, Error> {
    let servers = servers_to_check().await;

    let mut unreachable: Vec<String> = Vec::new();

    for server in &servers {
        assert_not_expired(expires)?;

        match force_get_user_on(&Target::Named(server.clone()), user, expires).await {
            Ok(Some(found)) => return Ok(Some(found)),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "Could not check whether user {} exists on FreeIPA server {}. Error: {}",
                    user,
                    server,
                    e
                );
                unreachable.push(server.clone());
            }
        }
    }

    if !unreachable.is_empty() {
        tracing::warn!(
            "REPLICATION-RISK: about to treat user {} as not existing, but {} of {} \
             FreeIPA servers could not be asked ({}). If the user does exist on one of \
             those, adding them now will create an LDAP replication conflict.",
            user,
            unreachable.len(),
            servers.len(),
            unreachable.join(", ")
        );
    }

    Ok(None)
}

///
/// Look for the group of the passed project on *every* configured FreeIPA
/// server, returning it from the first server that has it.
///
/// See `force_get_user_everywhere` for why this is asked of every master
/// rather than of whichever one the pool happened to hand us.
///
async fn force_get_group_everywhere(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAGroup>, Error> {
    let servers = servers_to_check().await;

    let mut unreachable: Vec<String> = Vec::new();

    for server in &servers {
        assert_not_expired(expires)?;

        match force_get_group_on(&Target::Named(server.clone()), project, expires).await {
            Ok(Some(found)) => return Ok(Some(found)),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "Could not check whether the group for project {} exists on FreeIPA \
                     server {}. Error: {}",
                    project,
                    server,
                    e
                );
                unreachable.push(server.clone());
            }
        }
    }

    if !unreachable.is_empty() {
        tracing::warn!(
            "REPLICATION-RISK: about to treat the group for project {} as not existing, \
             but {} of {} FreeIPA servers could not be asked ({}). If the group does exist \
             on one of those, adding it now will create an LDAP replication conflict.",
            project,
            unreachable.len(),
            servers.len(),
            unreachable.join(", ")
        );
    }

    Ok(None)
}

///
/// Return all of the groups that the user is a member of
///
async fn get_groups_for_user(
    user: &IPAUser,
    expires: &chrono::DateTime<Utc>,
) -> Result<Vec<IPAGroup>, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("user".to_string(), user.userid().to_string());
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("sizelimit".to_string(), "2048".to_string());
        kwargs
    };

    let result = call_post::<IPAResponse>("group_find", None, Some(kwargs), expires).await?;

    let internal_groups = cache::get_internal_group_ids().await?;

    let groups = result
        .groups()?
        .into_iter()
        .chain(result.internal_groups(&internal_groups)?)
        .chain(result.legacy_groups(&user.identifier().portal_identifier())?)
        .collect::<Vec<IPAGroup>>();

    // remove duplicates from this list
    let mut seen = HashSet::new();

    let groups = groups
        .into_iter()
        .filter(|g| seen.insert(g.groupid().to_string()))
        .collect::<Vec<IPAGroup>>();

    tracing::info!(
        "User {} is in groups: {:?}",
        user.identifier(),
        groups.iter().map(|g| g.groupid()).collect::<Vec<&str>>()
    );

    Ok(groups)
}

///
/// Get the UIDs of all direct members of a group via group_show.
/// Returns an empty set if the group does not exist in FreeIPA.
///
async fn get_instance_group_member_uids(
    group: &IPAGroup,
    expires: &chrono::DateTime<Utc>,
) -> Result<HashSet<String>, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs
    };

    match call_post::<IPAResponse>(
        "group_show",
        Some(vec![group.groupid().to_string()]),
        Some(kwargs),
        expires,
    )
    .await
    {
        Err(Error::NotFound(_)) => {
            tracing::warn!("Instance group {} not found in FreeIPA", group.groupid());
            Ok(HashSet::new())
        }
        Err(e) => Err(e),
        Ok(response) => {
            let raw = response.result.clone().unwrap_or_default();

            let uids: HashSet<String> = raw
                .get("member_user")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            tracing::debug!(
                "Instance group {} has {} direct member UIDs",
                group.groupid(),
                uids.len()
            );
            Ok(uids)
        }
    }
}

///
/// Extract memberuser UIDs from a raw FreeIPA group JSON object.
///
fn raw_group_member_uids(raw_group: &serde_json::Value) -> HashSet<String> {
    raw_group
        .get("member_user")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

///
/// Return the user matching the passed identifier - note that
/// this will only return users who are managed (part of the
/// "openportal" group)
///
async fn get_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<Option<IPAUser>, Error> {
    match cache::get_user(user).await? {
        Some(user) => Ok(Some(user.clone())),
        None => Ok(force_get_user(user, expires).await?),
    }
}

///
/// Call this function to get the group - adding it to FreeIPA if
/// it doesn't already exist
///
async fn get_group_create_if_not_exists(
    group: &IPAGroup,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAGroup, Error> {
    // If we already know this group exists then there is no decision to
    // serialise, and every add_user comes through here five times or so - for
    // the system groups, the instance groups, the managed group and the
    // project group - so the warm path must not queue behind the lock below.
    if let Some(group) = cache::get_group(group.identifier()).await? {
        return Ok(group);
    }

    // Take the lock for this group before looking, so that only one task at a
    // time can decide that this group needs creating. Two AddUser jobs for two
    // different users in the same project both come through here for the same
    // project group, and they are not duplicates of each other as far as the
    // job Board is concerned, so without this they both look, both see nothing
    // and both add - on whichever masters the pool handed them. That is the
    // largest single source of the namingConflict entries this guards against.
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_group_mutex(group.identifier())
            .await?
            .try_lock_owned()
        {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    tracing::warn!(
                        "Could not get lock to add group {} - another task is adding it.",
                        group
                    );

                    return Err(Error::Locked(format!(
                        "Could not get lock to add group {} - another task is adding it.",
                        group
                    )));
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    // another task may have created it while we were waiting for the lock
    if let Some(group) = cache::get_group(group.identifier()).await? {
        return Ok(group);
    }

    // Ask FreeIPA. This has to ask *every* master: a lookup served by one of
    // them says nothing about a group another one has just been given, and
    // "it does not exist" is the answer that makes us write.
    if let Some(existing) = force_get_group_everywhere(group.identifier(), expires).await? {
        tracing::info!(
            "Group {} already exists on another FreeIPA server - not creating it again.",
            existing
        );
        cache::add_existing_group(&existing).await?;
        return Ok(existing);
    }

    // it doesn't - try to create the group - we will encode the ProjectIdentifier
    // in the description
    let description = format!("{} | {}", group.identifier(), group.description());

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), group.groupid().to_string());
        kwargs.insert("description".to_string(), description);
        kwargs
    };

    match call_write::<IPAResponse>("group_add", None, Some(kwargs), expires).await {
        Ok(_) => {
            tracing::info!("Successfully created group: {}", group);
        }
        Err(Error::Duplicate(_)) => {
            // another writer got there first - that is a success, not a reason
            // to try again
            tracing::info!(
                "Group {} already existed when we tried to create it - continuing.",
                group
            );
        }
        Err(e) => {
            tracing::error!("Could not add group: {}. Error: {}", group, e);
        }
    }

    // The group should now exist in FreeIPA (either we added it, or another
    // writer beat us to it) - read it back as it actually is. This has to ask
    // every master rather than whichever one the pool hands us: the master
    // that served the add is the only one guaranteed to have it, and a
    // spurious "it isn't there" here would fail the job and invite the retry
    // that creates the second copy.
    match force_get_group_everywhere(group.identifier(), expires).await? {
        Some(group) => Ok(group),
        None => {
            tracing::error!("Failed to add group {} to FreeIPA", group);
            Err(Error::Call(format!(
                "Failed to add group {} to FreeIPA",
                group
            )))
        }
    }
}

///
/// Return the group that indicates that this user is managed
///
pub fn get_managed_group() -> Result<IPAGroup, Error> {
    IPAGroup::new(
        "openportal",
        &ProjectIdentifier::parse("openportal.openportal")?,
        "Group for all users managed by OpenPortal",
    )
}

///
/// Return the group that indicates that OpenPortal is managing this user
/// for the resource controlled by the passed Peer
///
pub fn get_op_instance_group(peer: &Peer) -> Result<IPAGroup, Error> {
    let group_name = format!("op-{}", peer);

    // make sure that the group name only contains letters and numbers,
    // replacing @ with $ and all other characters with _
    let group_name = group_name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' => c,
            //'@' => '$',
            _ => '_',
        })
        .collect::<String>();

    let id = ProjectIdentifier::parse(&format!("{}.instance", group_name))?;

    IPAGroup::new(
        &identifier_to_projectid(&id, false)?,
        &id,
        "Group for users in OpenPortal who access this instance",
    )
}

///
/// Return the name of the primary group for the user
///
fn get_primary_group(user: &UserIdentifier) -> Result<IPAGroup, Error> {
    let project = user.project_identifier();

    IPAGroup::new(
        &identifier_to_projectid(&project, false)?,
        &project,
        &format!(
            "Primary group for all users in the {} project",
            project.project()
        ),
    )
}

pub async fn get_primary_group_name(user: &UserIdentifier) -> Result<String, Error> {
    let group = get_primary_group(user)?;

    Ok(group.groupid().to_string())
}

///
/// Call this function to synchronise the groups for the passed user.
/// This checks that the user is in the correct groups, and adds them
/// or removes them as necessary. Groups will match the project group,
/// the system groups, and the openportal group.
///
///
/// Every group a fully-added, managed user must belong to on this instance.
///
/// Factored out of `sync_groups` so that `is_local_user_added` - which has to
/// decide whether an earlier `add_user` really finished - asks about exactly
/// the groups adding a user puts them in, and cannot drift from it.
///
async fn expected_groups(user: &UserIdentifier, instance: &Peer) -> Result<Vec<IPAGroup>, Error> {
    let mut groups = cache::get_system_groups().await?;

    // add in the groups for this instance
    groups.extend(cache::get_instance_groups(instance).await?);

    // add in the "openportal" group, to which all users managed by
    // OpenPortal must belong
    groups.push(get_managed_group()?);

    // also add in the group for the user's project (this is their primary group)
    groups.push(get_primary_group(user)?);

    Ok(groups)
}

async fn sync_groups(
    user: &IPAUser,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAUser, Error> {
    // the user probably doesn't exist, so add them, making sure they
    // are in the correct groups
    let groups = expected_groups(user.identifier(), instance).await?;

    // first step, make sure that all of the groups exist - and get their CNs
    let mut group_cns = Vec::new();

    for group in &groups {
        let added_group = get_group_create_if_not_exists(group, expires).await?;

        if group.identifier() != added_group.identifier() {
            tracing::error!(
                "Disagreement of identifier of added group: {} versus {}",
                group,
                added_group
            );

            return Err(Error::InvalidState(format!(
                "Disagreement of identifier of added group: {} versus {}",
                group, added_group
            )));
        }

        group_cns.push(added_group.groupid().to_string());
    }

    // return the user in the system - check that the groups match
    let user = get_user(user.identifier(), expires)
        .await?
        .ok_or(Error::Call(format!(
            "User {} could not be found after adding?",
            user
        )))?;

    // We cannot do anything to a user who isn't enabled
    if user.is_disabled() {
        tracing::error!(
            "Cannot sync groups for user {} as they are disabled in FreeIPA.",
            user.userid()
        );

        return Err(Error::UnmanagedUser(format!(
            "Cannot sync groups for user {} as they are disabled in FreeIPA.",
            user.userid()
        )));
    }

    // We cannot do anything to a user who isn't managed
    if !user.is_managed() {
        tracing::error!(
            "Cannot sync groups for user {} as they are not managed by OpenPortal.",
            user.userid()
        );

        return Err(Error::UnmanagedUser(format!(
            "Cannot sync groups for user {} as they are not managed by OpenPortal.",
            user.userid()
        )));
    }

    // which groups are missing?
    group_cns.retain(|group| !user.in_group_cn(group));

    let userid = user.userid().to_string();

    // now add the user to all of the groups
    for group_cn in &group_cns {
        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), group_cn.clone());
            kwargs.insert("user".to_string(), userid.clone());
            kwargs
        };

        match call_write::<IPAResponse>("group_add_member", None, Some(kwargs), expires).await {
            Ok(_) => tracing::info!("Successfully added user {} to group {}", userid, group_cn),
            Err(e) => {
                // this should not happen - it indicates that the group has disappeared
                // since we last updated. Our cache is now likely out of date.
                tracing::error!(
                    "Could not add user {} to group {}. Error: {}",
                    userid,
                    group_cn,
                    e
                );
                tracing::info!("Clearing the cache as FreeIPA has changed behind our back.");
                cache::clear().await?;
                // Return None so that the caller handles this failure case
                return Err(Error::InvalidState(format!(
                    "Could not add user {} to group {}. Error: {}. Likely freeipa was changed behind our back!",
                    userid, group_cn, e
                )));
            }
        }
    }

    // and also cache that this user is a member of the project groups
    let project_groups: Vec<IPAGroup> = groups
        .into_iter()
        .filter(|g| g.is_project_group())
        .collect();

    cache::add_user_to_groups(&user, &project_groups).await?;

    // finally - re-fetch the user from FreeIPA to make sure that we have
    // the correct information
    match force_get_user(user.identifier(), expires).await? {
        Some(user) => Ok(user),
        None => {
            tracing::warn!(
                "Failed to sync groups for user {} as this user no longer exists in FreeIPA.",
                user.identifier()
            );
            tracing::info!("Clearing the cache as FreeIPA has changed behind our back.");
            cache::clear().await?;
            // Return None so that the caller handles this failure case
            Err(Error::InvalidState(format!(
                "Failed to sync groups for user {} as this user no longer exists in FreeIPA. Likely freeipa was changed behind our back!",
                user.identifier()
            )))
        }
    }
}

///
/// Add the project to FreeIPA - this will create the group for the project
/// if it doesn't already exist. This returns the group
///
pub async fn add_project(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAGroup, Error> {
    // ensure that we don't have too many concurrent requests
    let project_group = get_group_create_if_not_exists(
        &IPAGroup::new(
            &identifier_to_projectid(project, false)?,
            project,
            "OpenPortal-managed group",
        )?,
        expires,
    )
    .await?;

    Ok(project_group)
}

///
/// Remove the project from FreeIPA - this will remove the group for the project
/// if it exists, returning the removed group if successful,
/// or it will return an error if it doesn't exist, or something else
/// goes wrong
///
pub async fn remove_project(
    project: &ProjectIdentifier,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAGroup, Error> {
    let project_group = match get_group(project, expires).await {
        Ok(Some(group)) => group,
        Ok(None) => {
            tracing::warn!(
                "Could not find group for project {}. Assuming it has already been removed.",
                project
            );
            return Err(Error::NotFound(format!(
                "Could not find group for project {}. Assuming it has already been removed.",
                project
            )));
        }
        Err(e) => {
            tracing::error!("Could not find group for project {}. Error: {}", project, e);
            return Err(Error::Call(format!(
                "Could not find group for project {}. Error: {}",
                project, e
            )));
        }
    };

    if !project_group.is_project_group() {
        return Err(Error::InvalidState(format!(
            "Cannot remove the group {} associated with project {} because it is not a project group?",
            project_group, project)));
    }

    assert_not_expired(expires)?;

    // now get all of the users in this project and remove them as well!
    let users = force_get_users_in_group(&Target::Any, &project_group, expires).await?;

    tracing::info!(
        "Removing group {} for project {}. Users to remove: {}",
        project_group.groupid(),
        project,
        users
            .iter()
            .map(|u| u.userid())
            .collect::<Vec<&str>>()
            .join(", ")
    );

    for user in users {
        assert_not_expired(expires)?;

        if !user.is_managed() {
            tracing::warn!(
                "Ignoring user {} as they are not managed by OpenPortal",
                user.userid()
            );
            continue;
        }

        match remove_user(user.identifier(), instance, expires).await {
            Ok(user) => {
                tracing::info!("Successfully removed group user: {}", user);
            }
            Err(e) => {
                tracing::error!(
                    "Could not remove user {} who is a member of project group {}. Error: {}",
                    user.userid(),
                    project_group.groupid(),
                    e
                );
            }
        };
    }

    // DO NOT REMOVE THE GROUP AS WE MAY WANT TO RE-ADD IT LATER, AND
    // WILL NEED TO USE THE SAME GID!

    Ok(project_group)
}

async fn reenable_user(user: &IPAUser, expires: &chrono::DateTime<Utc>) -> Result<IPAUser, Error> {
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs
    };

    match call_write::<IPAResponse>("user_enable", None, Some(kwargs), expires).await {
        Ok(_) => {
            let mut user = user.clone();
            user.set_enabled();
            tracing::info!("Successfully re-enabled user: {}", user.identifier());
            // re-add the user to the cache
            cache::add_existing_user(&user).await?;
            Ok(user)
        }
        Err(Error::NotFound(_)) => {
            tracing::warn!(
                "User {} not found in FreeIPA. They have been removed behind our back and cannot be enabled.",
                user.identifier()
            );

            // invalidate the cache, as FreeIPA has been changed behind our back
            cache::clear().await?;

            Err(Error::NotFound(format!(
                "User {} not found in FreeIPA. They have been removed behind our back and \
                 cannot be enabled.",
                user.identifier()
            )))
        }
        Err(e) => {
            tracing::error!("Could not enable user: {}. Error: {}", user, e);

            // invalidate the cache, as FreeIPA has been changed behind our back
            cache::clear().await?;

            Err(Error::Call(format!(
                "Could not enable user: {}. Error: {}",
                user, e
            )))
        }
    }
}

///
/// Add the passed user to FreeIPA, added from the passed peer instance.
/// This will return the added user if successful, or will return an
/// error if something goes wrong. This returns the existing user if
/// they are already in FreeIPA. Note that this will only work for
/// users that are managed by OpenPortal, i.e. there will be an error
/// if there is an exising user with the same name, but which is not
/// managed by OpenPortal
///
pub async fn add_user(
    user: &UserIdentifier,
    instance: &Peer,
    homedir: &Option<String>,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAUser, Error> {
    // get a lock for this user, as only a single task should be adding
    // or removing this user at the same time
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user).await?.try_lock_owned() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    tracing::warn!(
                        "Could not get lock to add user {} - another task is adding or removing.",
                        user
                    );

                    return Err(Error::Locked(format!(
                        "Could not get lock to add user {} - another task is adding or removing.",
                        user
                    )));
                }

                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    assert_not_expired(expires)?;

    // Return the up-to-date user if they already exist. This asks every
    // master, not just the one the pool hands us: this is the check that
    // decides whether we issue a `user_add`, and a master that has not yet
    // received a recent add would tell us the user does not exist.
    if let Some(mut user) = force_get_user_everywhere(user, expires).await? {
        assert_not_expired(expires)?;

        if !user.is_managed() {
            tracing::warn!(
                "Ignoring request to add {} as they are not managed by OpenPortal",
                user.identifier()
            );

            // make sure to add the user to the cache
            cache::add_existing_user(&user).await?;

            return Ok(user);
        }

        // blocked users must be explicitly unblocked - don't re-enable them here
        if user.is_blocked() {
            tracing::info!(
                "User {} is blocked - not re-enabling during add_user. Use unblock_user to unblock.",
                user.identifier()
            );
            cache::add_existing_user(&user).await?;
            return Ok(user);
        }

        // make sure to re-enable if needed
        if user.is_disabled() {
            user = match reenable_user(&user, expires).await {
                Ok(user) => user,
                Err(e) => {
                    tracing::error!(
                        "Could not re-enable user {} after adding. Error: {}",
                        user.identifier(),
                        e
                    );

                    // return the original user that is not enabled
                    user
                }
            }
        }

        if user.is_managed() && user.is_enabled() {
            // make sure that the groups are correct for the existing user
            match sync_groups(&user, instance, expires).await {
                Ok(user) => {
                    tracing::info!("Added user [cached] {}", user.identifier());
                    return Ok(user);
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to sync groups for user {} after adding. Error: {}",
                        user.identifier(),
                        e
                    );
                    tracing::info!(
                        "Will try to add user {} again, as the groups are not correct.",
                        user.identifier()
                    );
                }
            }
        }

        // we get here if either the user isn't in FreeIPA, or there was
        // some problem re-enabling them. This means we will fall through
        // and will try to add the user from scratch
    }

    assert_not_expired(expires)?;

    // Get the group that all managed users need to belong to
    let managed_group = get_managed_group()?;

    assert_not_expired(expires)?;

    // The user doesn't exist, so try to add
    let mut kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), identifier_to_userid(user).await?);
        kwargs.insert("givenname".to_string(), user.username().to_string());
        kwargs.insert("sn".to_string(), user.project().to_string());
        kwargs.insert("userclass".to_string(), managed_group.groupid().to_string());
        kwargs.insert("cn".to_string(), user.to_string());

        kwargs
    };

    if let Some(homedir) = homedir {
        kwargs.insert("homedirectory".to_string(), homedir.to_string());
        tracing::info!("Adding user {} with home directory: {}", user, homedir);
    }

    // we need to let the below go to completion, even if expired, as the
    // user needs to be removed if something goes wrong
    let user = match call_write::<IPAResponse>("user_add", None, Some(kwargs), expires).await {
        Ok(result) => {
            tracing::info!("Successfully added user: {}", user);
            result.users(&user.project_identifier())?.first().cloned().ok_or(Error::UnmanagedUser(format!(
                "User {} could not be found after adding - this could be because they already exist, but aren't managed? \
                 Look for the user in FreeIPA and either add them to the managed group or removed them from FreeIPA.",
                user
            )))?
        }
        Err(Error::Duplicate(_)) => {
            // failed to add because the user already exists
            tracing::warn!(
                "Cannot add user {} as FreeIPA thinks they already exist",
                user
            );
            cache::clear().await?;

            match force_get_user_everywhere(user, expires).await? {
                Some(mut user) => {
                    if user.is_blocked() {
                        // blocked users must be explicitly unblocked - don't re-enable them here
                        tracing::info!(
                            "User {} is blocked - not re-enabling during add_user. Use unblock_user to unblock.",
                            user.identifier()
                        );
                        cache::add_existing_user(&user).await?;
                        return Ok(user);
                    } else if user.is_disabled() {
                        if user.is_managed() {
                            // the user should be enabled...
                            user = reenable_user(&user, expires).await?;
                        } else {
                            tracing::warn!(
                                "User {} already exists in FreeIPA, but is not managed. \
                                Either add this user to the managed group, or remove them from FreeIPA.",
                                user
                            );

                            Err(Error::UnmanagedUser(
                                format!("User {} already exists in FreeIPA, but is not managed by OpenPortal. \
                                Either add this user to the managed group, or remove them from FreeIPA.", user)
                            ))?
                        }
                    }

                    user
                }
                None => {
                    tracing::warn!(
                        "Unable to fetch the user, despite them existing in FreeIPA. \
                            This is because the existing user is not managed. Either add \
                            this user to the managed group, or remove them from FreeIPA."
                    );

                    Err(Error::UnmanagedUser(
                        format!("User {} already exists in FreeIPA, but is not managed by OpenPortal. \
                                 Either add this user to the managed group, or remove them from FreeIPA.", user)
                    ))?
                }
            }
        }
        Err(e) => {
            // The add failed - but a timeout or a dropped response is
            // indistinguishable here from a rejected write, and the add may
            // well have landed. Ask every master before deciding, because
            // treating a completed add as a failure is what makes the caller
            // retry and create the second copy.
            tracing::error!("Could not add user: {}. Error: {}", user, e);
            match force_get_user_everywhere(user, expires).await? {
                Some(user) => {
                    tracing::debug!("User already exists: {}", user);
                    user
                }
                None => {
                    return Err(Error::Call(format!(
                        "Could not add user: {}. Error: {}",
                        user, e
                    )));
                }
            }
        }
    };

    // add this user to the managed group so that it can be managed
    let userid = user.userid().to_string();

    match loop {
        // make sure that this group exists
        let managed_group = get_group_create_if_not_exists(&managed_group, expires).await?;

        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), managed_group.groupid().to_string());
            kwargs.insert("user".to_string(), userid.clone());
            kwargs
        };

        match call_write::<IPAResponse>("group_add_member", None, Some(kwargs), expires).await {
            Ok(_) => {
                break Ok(());
            }
            Err(Error::NotFound(_)) => {
                tracing::warn!(
                    "Group {} not found in FreeIPA. Assuming it has been removed - clearing cache and re-adding.",
                    managed_group
                );
                cache::clear().await?;
            }
            Err(e) => {
                break Err(e);
            }
        }
    } {
        Ok(_) => {
            tracing::info!(
                "Successfully added user {} to group {}",
                userid,
                managed_group
            );
        }
        Err(e) => {
            tracing::error!(
                "Could not add user {} to group {}. Error: {}",
                userid,
                managed_group,
                e
            );

            // this failed, so we need to remove the user so that we can try again
            // BUT we can only remove the user if they aren't in any other instance groups...
            for group in get_groups_for_user(&user, expires).await? {
                if group.is_instance_group() {
                    tracing::warn!(
                        "User {} is in instance group {}. Cannot remove user after failed add.",
                        user.userid(),
                        group.groupid()
                    );
                    return Err(Error::UnmanagedUser(format!(
                        "User {} already exists in FreeIPA, but could not be added to the managed group. \
                        They are in the instance group {}. Either remove them from this group, or try again later.",
                        user.userid(),
                        group.groupid()
                    )));
                }
            }

            let kwargs = {
                let mut kwargs = HashMap::new();
                kwargs.insert("uid".to_string(), userid.clone());
                kwargs
            };

            match call_write::<IPAResponse>("user_disable", None, Some(kwargs), expires).await {
                Ok(_) => {
                    tracing::info!(
                        "Successfully removed user {} after failed group add",
                        userid
                    )
                }
                Err(e) => {
                    tracing::error!(
                        "Could not remove user {} after failed group add. Error: {}",
                        userid,
                        e
                    );
                }
            }

            return Err(Error::Call(format!(
                "Could not add user {} to group {}. Error: {}",
                user, managed_group, e
            )));
        }
    }

    // now synchronise the groups
    let mut attempts = 0;

    match loop {
        attempts += 1;

        if attempts > 3 {
            break Err(Error::Call(format!(
                "Failed to synchronise groups for user {} after 3 attempts",
                user.identifier()
            )));
        }

        match sync_groups(&user, instance, expires).await {
            Ok(user) => {
                tracing::info!("Added user: {}", user);
                break Ok(user);
            }
            Err(Error::NotFound(e)) => {
                tracing::warn!(
                "User {} or groups not found in FreeIPA. They have been removed? Clearing cache and re-adding. Error: {}",
                user, e
            );
                cache::clear().await?;
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to synchronise groups for user {}: {}",
                    user.identifier(),
                    e
                );
                break Err(Error::Call(format!(
                    "Failed to synchronise groups for user {}: {}",
                    user.identifier(),
                    e
                )));
            }
        }
    } {
        Ok(user) => Ok(user),
        Err(e) => Err(e),
    }
}

///
/// Remove the user from FreeIPA - this will return the removed user if
/// successful, or will return an error if the user doesn't exist, or
/// something else goes wrong. Note that the user must be managed by
/// OpenPortal, or an error will be returned
///
pub async fn remove_user(
    user: &UserIdentifier,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAUser, Error> {
    // get and lock a mutex on this user, as we should only have a single
    // task adding or removing this user at once
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user).await?.try_lock_owned() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    tracing::warn!(
                        "Could not get lock to remove user {} - another task is adding or removing.",
                        user
                    );

                    return Err(Error::Locked(format!(
                        "Could not get lock to remove user {} - another task is adding or removing.",
                        user
                    )));
                }

                assert_not_expired(expires)?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    assert_not_expired(expires)?;

    // force get this user, as we need to have up-to-date information from FreeIPA
    let mut user = match force_get_user(user, expires).await {
        Ok(Some(user)) => user,
        Ok(None) => {
            tracing::warn!(
                "Could not find user {}. Assuming they have already been removed.",
                user
            );
            return Err(Error::NotFound(format!(
                "Could not find user {}. Assuming they have already been removed.",
                user
            )));
        }
        Err(e) => {
            tracing::error!("Could not find user {}. Error: {}", user, e);
            return Err(Error::Call(format!(
                "Could not find user {}. Error: {}",
                user, e
            )));
        }
    };

    if !user.is_managed() {
        tracing::warn!(
            "Ignoring request to remove {} as they are not managed by OpenPortal",
            user.identifier()
        );
        return Err(Error::UnmanagedUser(format!(
            "User {} is not managed by OpenPortal. Either add this user to the managed group, or remove them from FreeIPA.",
            user.identifier()
        )));
    }

    if user.is_disabled() && !user.is_blocked() {
        // nothing to do - user has already been removed
        tracing::info!(
            "User {} is already disabled. No changes needed.",
            user.identifier()
        );
        return Ok(user);
    }

    // get the group used for openportal users of this peer
    let instance_group = get_op_instance_group(instance)?;

    // maybe don't do anything if the user isn't a member of this group
    if !user.in_group(&instance_group) {
        // check that they are in any groups...
        let in_other_instance_groups = match get_groups_for_user(&user, expires).await {
            Ok(groups) => !groups
                .iter()
                .filter(|g| g.is_instance_group())
                .filter(|g| g.identifier() != instance_group.identifier())
                .collect::<Vec<&IPAGroup>>()
                .is_empty(),
            Err(e) => {
                tracing::error!("Could not get groups for user {}. Error: {}", user, e);
                false
            }
        };

        if in_other_instance_groups {
            tracing::warn!(
                "Ignoring request to remove {} as they are not in the instance group {}, but are in other resources",
                user.identifier(),
                instance_group.identifier(),
            );
            return Ok(user);
        }
    }

    assert_not_expired(expires)?;

    // don't check for expiry below as this has to run to completion

    // now remove the user from all of the instance groups for this peer
    let instance_groups = cache::get_instance_groups(instance).await?;

    for group in instance_groups {
        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), group.groupid().to_string());
            kwargs.insert("user".to_string(), user.userid().to_string());
            kwargs
        };

        match call_write::<IPAResponse>("group_remove_member", None, Some(kwargs), expires).await {
            Ok(_) => {
                tracing::info!(
                    "Successfully removed user {} from group {}",
                    user.identifier(),
                    group.groupid()
                );
            }
            Err(Error::NotFound(_)) => {
                tracing::info!(
                    "Group {} not found in FreeIPA. Assuming it has already been removed.",
                    group
                );

                cache::clear().await?;
            }
            Err(e) => {
                tracing::error!(
                    "Could not remove user {} from group {}. Error: {}",
                    user.identifier(),
                    group.groupid(),
                    e
                );
            }
        }
    }

    // refetch the groups for this user, as they will have changed
    let groups = match get_groups_for_user(&user, expires).await {
        Ok(groups) => groups,
        Err(e) => {
            tracing::error!("Could not get groups for user {}. Error: {}", user, e);
            vec![]
        }
    };

    // get all of the other instance groups for this user
    let other_instance_groups = groups
        .iter()
        .filter(|g| g.is_instance_group())
        .filter(|g| g.identifier() != instance_group.identifier())
        .collect::<Vec<&IPAGroup>>();

    // don't remove the user if they are on different resources
    if !other_instance_groups.is_empty() {
        tracing::warn!(
            "Ignoring request to remove {} as they are in other resources: {:?}",
            user.identifier(),
            other_instance_groups
                .iter()
                .map(|g| g.identifier().to_string())
                .collect::<Vec<String>>()
        );

        // remove this user from the cache so that the list of users in this
        // project for this resource will be properly updated
        cache::remove_existing_user(&user).await?;

        return Ok(user);
    }

    // it is safe to remove the user - they aren't in any other resource

    // remove the user from all groups EXCEPT the managed group
    // This is necessary to make sure that we don't accidentally
    // add the user back to groups they don't have permission to be
    // in if they are re-enabled
    let managed_group = get_managed_group()?;

    for group in groups {
        if group.identifier() == managed_group.identifier() {
            continue;
        }

        let kwargs = {
            let mut kwargs = HashMap::new();
            kwargs.insert("cn".to_string(), group.groupid().to_string());
            kwargs.insert("user".to_string(), user.userid().to_string());
            kwargs
        };

        match call_write::<IPAResponse>("group_remove_member", None, Some(kwargs), expires).await {
            Ok(_) => {
                tracing::info!(
                    "Successfully removed user {} from group {}",
                    user.identifier(),
                    group.groupid()
                );
            }
            Err(Error::NotFound(_)) => {
                tracing::info!(
                    "Group {} not found in FreeIPA. Assuming it has already been removed.",
                    group
                );

                cache::clear().await?;
            }
            Err(e) => {
                tracing::error!(
                    "Could not remove user {} from group {}. Error: {}",
                    user.identifier(),
                    group.groupid(),
                    e
                );
            }
        }
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs
    };

    // we don't actually remove users - instead we disable them so that
    // they can't log in. This way, if the user is re-added, then they
    // will get the same UID and other details
    match call_write::<IPAResponse>("user_disable", None, Some(kwargs), expires).await {
        Ok(_) => {
            user.set_disabled();
            tracing::info!("Successfully removed user: {}", user.identifier());
            cache::remove_existing_user(&user).await?;
        }
        Err(Error::NotFound(_)) => {
            tracing::info!(
                "User {} not found in FreeIPA. Assuming it has already been removed.",
                user.identifier()
            );

            // clear the cache as FreeIPA has been changed behind our back
            cache::clear().await?;
        }
        Err(e) => {
            tracing::error!("Could not remove user: {}. Error: {}", user.identifier(), e);
            return Err(Error::Call(format!(
                "Could not remove user: {}. Error: {}",
                user.identifier(),
                e
            )));
        }
    }

    Ok(user)
}

///
/// Update the homedir for the user - this will return the updated homedir
/// if successful, or will return an error if the user doesn't exist, or
/// something else goes wrong. Note that the user must be managed by
/// OpenPortal, or an error will be returned
///
pub async fn update_homedir(
    user: &UserIdentifier,
    homedir: &str,
    expires: &chrono::DateTime<Utc>,
) -> Result<String, Error> {
    let homedir = homedir.trim();

    if homedir.is_empty() {
        return Err(Error::InvalidState("Empty homedir".to_string()));
    }

    // get the user from FreeIPA
    let user = get_user(user, expires).await?.ok_or(Error::Call(format!(
        "User {} does not exist in FreeIPA?",
        user
    )))?;

    assert_not_expired(expires)?;

    if !user.is_managed() {
        tracing::warn!(
            "Ignoring request to update homedir for {} as they are not managed by OpenPortal",
            user.identifier()
        );
        return Ok(user.home().to_string());
    }

    if user.home() == homedir {
        // nothing to do
        tracing::debug!(
            "Homedir for user {} is already {}. No changes needed.",
            user.identifier(),
            homedir
        );
        return Ok(user.home().to_string());
    }

    assert_not_expired(expires)?;

    // do not check for expiry below as this has to run to completion

    // now update the homedir to the passed string
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs.insert("homedirectory".to_string(), homedir.to_string());
        kwargs
    };

    match call_write::<IPAResponse>("user_mod", None, Some(kwargs), expires).await {
        Ok(_) => {
            tracing::info!(
                "Successfully updated homedir for user: {}",
                user.identifier()
            );
        }
        Err(Error::NotFound(_)) => {
            tracing::info!(
                "User {} not found in FreeIPA. Assuming it has been removed behind our back.",
                user
            );

            // clear the cache as FreeIPA has been changed behind our back
            cache::clear().await?;
        }
        Err(e) => {
            tracing::error!(
                "Could not update homedir for user {} to {}. Error: {}",
                user.identifier(),
                homedir,
                e
            );
        }
    }

    // now update the user in the cache
    let user = force_get_user(user.identifier(), expires)
        .await?
        .ok_or(Error::Call(format!(
            "User {} does not exist in FreeIPA?",
            user.identifier()
        )))?;

    if user.home() != homedir {
        return Err(Error::InvalidState(format!(
            "Homedir for user {} was not updated to {}",
            user, homedir
        )));
    }

    tracing::info!("User homedir updated: {}", user);

    Ok(user.home().to_string())
}

///
/// Return all of the groups that are managed by OpenPortal for the
/// passed portal
///
pub async fn get_groups(
    portal: &PortalIdentifier,
    peer: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<Vec<IPAGroup>, Error> {
    tracing::debug!("Getting managed groups for portal: {}", portal);
    if is_internal_portal(&portal.portal()) {
        // return an empty set of groups for internal portals
        return Ok(Vec::new());
    }

    // Strategy: search for groups by portal name prefix, then filter to those
    // that have at least one member in the instance group. This avoids enumerating
    // all users on the system and performing N+1 group lookups per user.
    let instance_group = get_op_instance_group(peer)?;

    // Step 1: get the UIDs of all direct members of the instance group (single API call,
    // no full user object construction needed)
    tracing::info!(
        "Getting members of instance group '{}' for portal {}",
        instance_group.groupid(),
        portal
    );

    let instance_members = get_instance_group_member_uids(&instance_group, expires).await?;

    assert_not_expired(expires)?;

    if instance_members.is_empty() {
        tracing::warn!(
            "No users in instance group '{}' for portal {}",
            instance_group.groupid(),
            portal
        );
        return Ok(Vec::new());
    }

    tracing::info!(
        "Instance group '{}' has {} members for portal {}",
        instance_group.groupid(),
        instance_members.len(),
        portal
    );

    // Step 2: find all groups for this portal using the server-side name prefix filter.
    // Group names are "{portal}.{project}", so passing "{portal}." as the positional
    // search criteria restricts FreeIPA's substring match to only this portal's groups.
    // Note: cn must be a positional arg (not a kwarg) for FreeIPA to use substring matching;
    // passing it as a kwarg triggers exact matching.
    let portal_prefix = format!("{}.", portal.portal());
    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("sizelimit".to_string(), "4096".to_string());
        kwargs
    };

    let result = call_post::<IPAResponse>(
        "group_find",
        Some(vec![portal_prefix]),
        Some(kwargs),
        expires,
    )
    .await?;

    if result.truncated.unwrap_or(false) {
        tracing::warn!(
            "group_find for portal {} was truncated at 4096 results; \
             some groups may be missing.",
            portal
        );
    }

    // Step 3: filter to groups that have at least one member in the instance group,
    // then construct IPAGroup objects from the filtered raw entries
    let raw_groups = result.result.clone().unwrap_or_default();
    let raw_groups = raw_groups.as_array().cloned().unwrap_or_default();

    let filtered: Vec<serde_json::Value> = raw_groups
        .into_iter()
        .filter(|raw| {
            let members = raw_group_member_uids(raw);
            members.iter().any(|uid| instance_members.contains(uid))
        })
        .collect();

    let mut groups = IPAGroup::construct(&serde_json::Value::Array(filtered))?;

    assert_not_expired(expires)?;

    // Step 4: also search for legacy groups (named "group.{project}").
    // Only include those whose description encodes a ProjectIdentifier for this portal
    // AND which have at least one member in the instance group.
    let legacy_kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("all".to_string(), "true".to_string());
        kwargs.insert("sizelimit".to_string(), "4096".to_string());
        kwargs
    };

    let legacy_result = call_post::<IPAResponse>(
        "group_find",
        Some(vec!["group.".to_string()]),
        Some(legacy_kwargs),
        expires,
    )
    .await?;

    if legacy_result.truncated.unwrap_or(false) {
        tracing::warn!(
            "group_find for legacy groups was truncated at 4096 results; \
             some legacy groups may be missing."
        );
    }

    let raw_legacy = legacy_result.result.clone().unwrap_or_default();
    let raw_legacy = raw_legacy.as_array().cloned().unwrap_or_default();

    let filtered_legacy: Vec<serde_json::Value> = raw_legacy
        .into_iter()
        .filter(|raw| {
            let members = raw_group_member_uids(raw);
            members.iter().any(|uid| instance_members.contains(uid))
        })
        .collect();

    let legacy_groups =
        IPAGroup::construct_legacy(&serde_json::Value::Array(filtered_legacy), portal)?;

    for group in legacy_groups {
        if !groups
            .iter()
            .any(|g: &IPAGroup| g.identifier() == group.identifier())
        {
            groups.push(group);
        }
    }

    tracing::info!(
        "Got groups for portal {}: {}",
        portal,
        groups
            .iter()
            .map(|g| g.groupid())
            .collect::<Vec<&str>>()
            .join(", ")
    );

    Ok(groups)
}

///
/// Return all of the users that are managed by OpenPortal for the
/// passed project. Note that this will only return users who are
/// managed by OpenPortal
///
pub async fn get_users(
    project: &ProjectIdentifier,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<Vec<IPAUser>, Error> {
    tracing::debug!("Getting users for project: {}", project);
    // don't get the users for project identifiers that use internal portal names
    // as they aren't public
    if is_internal_portal(&project.portal()) {
        return Ok(Vec::new());
    }

    let project_group = match get_group(project, expires).await {
        Ok(Some(group)) => group,
        Ok(None) => {
            tracing::warn!(
                "Could not find group for project {}. Assuming it has already been removed.",
                project
            );
            return Ok(vec![]);
        }
        Err(e) => {
            tracing::error!("Could not find group for project {}. Error: {}", project, e);
            return Err(Error::Call(format!(
                "Could not find group for project {}. Error: {}",
                project, e
            )));
        }
    };

    assert_not_expired(expires)?;

    // now collect all of the groups that the users would need to belong
    // to for them to be managed by OpenPortal in this project
    let mut required_groups = cache::get_system_groups().await?;

    // add in the groups for this instance
    required_groups.extend(cache::get_instance_groups(instance).await?);

    // add in the "openportal" group, to which all users managed by
    // OpenPortal must belong
    required_groups.push(get_managed_group()?);

    // now get all users in the project group
    let cached_users = cache::get_users_in_group(&project_group).await?;

    if !cached_users.is_empty() {
        // filter out users who are not in all required groups for this peer
        let users = cached_users
            .into_iter()
            .filter(|user| user.is_protected() || user.in_all_groups(&required_groups))
            .collect::<Vec<IPAUser>>();

        return Ok(users);
    }

    // there are no users, meaning that we have not checked yet, or there
    // really are no users in this project...
    let users = force_get_users_in_group(&Target::Any, &project_group, expires).await?;

    cache::set_users_in_group(&project_group, &users).await?;

    assert_not_expired(expires)?;

    // filter out users who are not in all required groups for this peer
    let users = users
        .into_iter()
        .filter(|user| user.is_protected() || user.in_all_groups(&required_groups))
        .collect::<Vec<IPAUser>>();

    Ok(users)
}

pub async fn get_project_mapping(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<ProjectMapping, Error> {
    match get_group(project, expires).await? {
        Some(group) => group.mapping(),
        None => Err(Error::MissingProject(format!(
            "Project {} does not exist in FreeIPA",
            project
        ))),
    }
}

pub async fn get_user_mapping(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<UserMapping, Error> {
    match get_user(user, expires).await? {
        Some(user) => user.mapping(),
        None => Err(Error::MissingUser(format!(
            "User {} does not exist in FreeIPA",
            user
        ))),
    }
}

pub async fn is_protected_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    // need to get the up-to-date version of the user,
    // in case their details have been changed in FreeIPA
    // behind our back. Important that we don't say a user
    // isn't protected when they have been manually removed from
    // the managed group...
    match force_get_user(user, expires).await? {
        Some(user) => Ok(!user.is_managed()),
        None => Ok(false),
    }
}

pub async fn is_existing_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    match get_user(user, expires).await? {
        Some(user) => {
            if user.is_enabled() || user.is_protected() || user.is_blocked() {
                Ok(true)
            } else {
                Ok(false)
            }
        }
        None => Ok(false),
    }
}

pub async fn is_existing_project(
    project: &ProjectIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    match get_group(project, expires).await? {
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

///
/// Ensure the "openportal.blocked" group exists in FreeIPA, creating it if
/// necessary. This group is the source of truth for which users are blocked.
///
async fn ensure_blocked_group_exists(expires: &chrono::DateTime<Utc>) -> Result<(), Error> {
    let blocked_cn = "openportal.blocked";

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), blocked_cn.to_string());
        kwargs.insert(
            "description".to_string(),
            "Group for all users blocked by OpenPortal".to_string(),
        );
        kwargs
    };

    match call_write::<IPAResponse>("group_add", None, Some(kwargs), expires).await {
        Ok(_) => {
            tracing::info!("Created {} group in FreeIPA", blocked_cn);
            Ok(())
        }
        Err(Error::Duplicate(_)) => {
            // group already exists - that's fine
            Ok(())
        }
        Err(e) => {
            tracing::error!("Failed to ensure {} group exists: {}", blocked_cn, e);
            Err(e)
        }
    }
}

pub async fn is_blocked_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    match force_get_user(user, expires).await? {
        Some(user) => Ok(user.is_blocked()),
        None => Ok(false),
    }
}

///
/// Block the user in FreeIPA by adding them to the "openportal.blocked" group
/// and disabling their account. This prevents login without removing their
/// account, home directory, or scheduler configuration. Only unblock_user
/// should re-enable them.
///
pub async fn block_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAUser, Error> {
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user).await?.try_lock_owned() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    return Err(Error::Locked(format!(
                        "Could not get lock to block user {} - another task is adding or removing.",
                        user
                    )));
                }
                assert_not_expired(expires)?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    assert_not_expired(expires)?;

    let user = match force_get_user(user, expires).await? {
        Some(user) => user,
        None => {
            return Err(Error::NotFound(format!(
                "Could not find user {} to block.",
                user
            )));
        }
    };

    if !user.is_managed() {
        return Err(Error::UnmanagedUser(format!(
            "User {} is not managed by OpenPortal - cannot block.",
            user.identifier()
        )));
    }

    if user.is_blocked() {
        tracing::info!(
            "User {} is already blocked - nothing to do.",
            user.identifier()
        );
        return Ok(user);
    }

    assert_not_expired(expires)?;

    ensure_blocked_group_exists(expires).await?;

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), "openportal.blocked".to_string());
        kwargs.insert("user".to_string(), user.userid().to_string());
        kwargs
    };

    match call_write::<IPAResponse>("group_add_member", None, Some(kwargs), expires).await {
        Ok(_) => {
            tracing::info!(
                "Added user {} to openportal.blocked group",
                user.identifier()
            );
        }
        Err(e) => {
            tracing::error!(
                "Could not add user {} to openportal.blocked group: {}",
                user.identifier(),
                e
            );
            return Err(e);
        }
    }

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("uid".to_string(), user.userid().to_string());
        kwargs
    };

    let mut user = user;
    match call_write::<IPAResponse>("user_disable", None, Some(kwargs), expires).await {
        Ok(_) => {
            user.set_disabled();
            tracing::info!("Blocked user: {}", user.identifier());
            cache::remove_existing_user(&user).await?;
        }
        Err(Error::NotFound(_)) => {
            tracing::warn!(
                "User {} not found when disabling during block - they may have been removed.",
                user.identifier()
            );
            cache::clear().await?;
        }
        Err(e) => {
            tracing::error!(
                "Could not disable user {} when blocking: {}",
                user.identifier(),
                e
            );
            return Err(e);
        }
    }

    Ok(user)
}

///
/// Unblock a previously blocked user by removing them from "openportal.blocked"
/// and re-enabling their account.
///
pub async fn unblock_user(
    user: &UserIdentifier,
    expires: &chrono::DateTime<Utc>,
) -> Result<IPAUser, Error> {
    let now = chrono::Utc::now();

    let _guard = loop {
        match cache::get_user_mutex(user).await?.try_lock_owned() {
            Ok(guard) => break guard,
            Err(_) => {
                if chrono::Utc::now().signed_duration_since(now).num_seconds() > 5 {
                    return Err(Error::Locked(format!(
                        "Could not get lock to unblock user {} - another task is adding or removing.",
                        user
                    )));
                }
                assert_not_expired(expires)?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        };
    };

    assert_not_expired(expires)?;

    let user = match force_get_user(user, expires).await? {
        Some(user) => user,
        None => {
            return Err(Error::NotFound(format!(
                "Could not find user {} to unblock.",
                user
            )));
        }
    };

    if !user.is_managed() {
        return Err(Error::UnmanagedUser(format!(
            "User {} is not managed by OpenPortal - cannot unblock.",
            user.identifier()
        )));
    }

    if !user.is_blocked() {
        tracing::info!("User {} is not blocked - nothing to do.", user.identifier());
        return Ok(user);
    }

    assert_not_expired(expires)?;

    let kwargs = {
        let mut kwargs = HashMap::new();
        kwargs.insert("cn".to_string(), "openportal.blocked".to_string());
        kwargs.insert("user".to_string(), user.userid().to_string());
        kwargs
    };

    match call_write::<IPAResponse>("group_remove_member", None, Some(kwargs), expires).await {
        Ok(_) => {
            tracing::info!(
                "Removed user {} from openportal.blocked group",
                user.identifier()
            );
        }
        Err(e) => {
            tracing::error!(
                "Could not remove user {} from openportal.blocked group: {}",
                user.identifier(),
                e
            );
            return Err(e);
        }
    }

    let user = reenable_user(&user, expires).await?;

    tracing::info!("Unblocked user: {}", user.identifier());

    Ok(user)
}

///
/// Return whether everything `add_user` does for this mapping has been done.
///
/// FreeIPA is asked directly (`force_get_user`) rather than through the cache:
/// the whole point of the question is to find out whether the directory really
/// is in the state an earlier `add_user` claimed to leave it in, and a cache
/// would just replay that claim back.
///
pub async fn is_local_user_added(
    mapping: &UserMapping,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    let user = match force_get_user(mapping.user(), expires).await? {
        Some(user) => user,
        None => {
            tracing::info!("User {} does not exist in FreeIPA", mapping.user());
            return Ok(false);
        }
    };

    // An unmanaged user is one `add_user` deliberately leaves exactly as it
    // found them and reports success for, so "has the add finished?" is
    // vacuously yes - there was never anything for us to do.
    if !user.is_managed() {
        tracing::info!(
            "User {} is not managed by OpenPortal - nothing for add_user to do",
            mapping.user()
        );
        return Ok(true);
    }

    // A blocked user is neither added nor removed: `add_user` refuses to
    // re-enable them, and only `unblock_user` will. `is_blocked_user` is the
    // instruction that tells a caller that.
    if user.is_blocked() {
        tracing::info!("User {} is blocked, so has not been added", mapping.user());
        return Ok(false);
    }

    if !user.is_enabled() {
        tracing::info!("User {} is disabled, so has not been added", mapping.user());
        return Ok(false);
    }

    let actual = user.mapping()?;

    if actual != *mapping {
        tracing::info!(
            "User {} exists, but maps to {} rather than the requested {}",
            mapping.user(),
            actual,
            mapping
        );
        return Ok(false);
    }

    let groups = expected_groups(mapping.user(), instance).await?;

    if !user.in_all_groups(&groups) {
        tracing::info!(
            "User {} is not yet in all of the groups they should be: {:?}",
            mapping.user(),
            groups
                .iter()
                .filter(|g| !user.in_group(g))
                .map(|g| g.groupid().to_string())
                .collect::<Vec<String>>()
        );
        return Ok(false);
    }

    Ok(true)
}

///
/// Return whether everything `remove_user` does for this mapping has been done.
///
/// `remove_user` disables the account and takes it out of this instance's
/// groups; it never deletes it, so "removed" is a state the user is left in
/// rather than their absence. It is also distinct from "blocked", which is also
/// disabled - the blocked group is what tells the two apart, and it is the same
/// distinction `remove_user` itself makes before deciding it has nothing to do.
///
pub async fn is_local_user_removed(
    mapping: &UserMapping,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    let user = match force_get_user(mapping.user(), expires).await? {
        Some(user) => user,
        None => return Ok(true),
    };

    // As in `is_local_user_added`: `remove_user` refuses to touch a user it
    // does not manage, so there is nothing outstanding for it to do.
    if !user.is_managed() {
        tracing::info!(
            "User {} is not managed by OpenPortal - nothing for remove_user to do",
            mapping.user()
        );
        return Ok(true);
    }

    if user.is_blocked() {
        tracing::info!("User {} is blocked, not removed", mapping.user());
        return Ok(false);
    }

    if user.is_enabled() {
        tracing::info!(
            "User {} is still enabled, so has not been removed",
            mapping.user()
        );
        return Ok(false);
    }

    // Disabled is not enough on its own - `remove_user` also strips this
    // instance's groups, and a user still in one of them can be re-added to
    // this resource by a `sync_groups` that has not run yet.
    for group in cache::get_instance_groups(instance).await? {
        if user.in_group(&group) {
            tracing::info!(
                "User {} is still in instance group {}, so has not been fully removed",
                mapping.user(),
                group.groupid()
            );
            return Ok(false);
        }
    }

    Ok(true)
}

///
/// Return whether everything `add_project` does for this mapping has been done,
/// i.e. the project's group exists in FreeIPA with the name the mapping says it
/// should have. Read straight from FreeIPA rather than from the cache.
///
pub async fn is_local_project_added(
    mapping: &ProjectMapping,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    let group = match force_get_group_on(&Target::Any, mapping.project(), expires).await? {
        Some(group) => group,
        None => {
            tracing::info!(
                "Group for project {} does not exist in FreeIPA",
                mapping.project()
            );
            return Ok(false);
        }
    };

    if !group.is_project_group() {
        tracing::warn!(
            "Group {} for project {} is not a project group",
            group,
            mapping.project()
        );
        return Ok(false);
    }

    let actual = group.mapping()?;

    if actual != *mapping {
        tracing::info!(
            "Project {} exists, but maps to {} rather than the requested {}",
            mapping.project(),
            actual,
            mapping
        );
        return Ok(false);
    }

    Ok(true)
}

///
/// Return whether everything `remove_project` does for this mapping has been
/// done. The group itself is deliberately kept - it holds the gid, which has to
/// stay stable if the project is ever re-added - so what removal actually means
/// here is that every managed user who was in it has been removed.
///
pub async fn is_local_project_removed(
    mapping: &ProjectMapping,
    instance: &Peer,
    expires: &chrono::DateTime<Utc>,
) -> Result<bool, Error> {
    let group = match force_get_group_on(&Target::Any, mapping.project(), expires).await? {
        Some(group) => group,
        None => return Ok(true),
    };

    if !group.is_project_group() {
        tracing::warn!(
            "Group {} for project {} is not a project group",
            group,
            mapping.project()
        );
        return Ok(true);
    }

    for user in force_get_users_in_group(&Target::Any, &group, expires).await? {
        // `remove_project` skips unmanaged users, so they are not evidence
        // that it has not finished.
        if !user.is_managed() {
            continue;
        }

        if !is_local_user_removed(&user.mapping()?, instance, expires).await? {
            tracing::info!(
                "Project {} still has user {} who has not been removed",
                mapping.project(),
                user.identifier()
            );
            return Ok(false);
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    ///
    /// One test for the whole server pool, because it is global state - as
    /// separate tests these would race each other.
    ///
    #[tokio::test]
    async fn test_server_pool() {
        let password = SecretString::from("secret");
        let ipa1 = "https://ipa1.example.com".to_string();
        let ipa2 = "https://ipa2.example.com".to_string();

        let init = |servers: Vec<String>, write_server: &'static str, window: Option<i64>| {
            let password = password.clone();
            async move {
                initialise_servers(&servers, write_server, window, None, "admin", &password).await
            }
        };

        // The `freeipa-server` option may list the same server several times,
        // which is how the pool is given more than one concurrent slot for it.
        // Those repeats are still one server as far as replication is
        // concerned, so the per-master existence checks must ask it once.
        match init(
            vec![ipa1.clone(), ipa2.clone(), ipa1.clone(), "  ".to_string()],
            "",
            None,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => unreachable!("initialise_servers: {:?}", e),
        }

        assert_eq!(configured_servers().await, vec![ipa1.clone(), ipa2.clone()]);

        // With no explicit write server, writes go to the first one configured
        assert_eq!(WRITE_CONFIG.lock().await.server.clone(), Some(ipa1.clone()));
        assert_eq!(
            WRITE_CONFIG.lock().await.replication_window.num_seconds(),
            DEFAULT_REPLICATION_WINDOW
        );

        // A write server that is not in the pool is a configuration error, not
        // something to quietly fall back from
        assert!(init(vec![ipa1.clone()], "https://ipa3.example.com", None)
            .await
            .is_err());

        // Name one explicitly, and it is asked about first when checking
        // whether something exists - it is the server holding our own writes
        match init(
            vec![ipa1.clone(), ipa2.clone()],
            "https://ipa2.example.com",
            Some(60),
        )
        .await
        {
            Ok(_) => {}
            Err(e) => unreachable!("initialise_servers: {:?}", e),
        }

        assert_eq!(servers_to_check().await, vec![ipa2.clone(), ipa1.clone()]);

        // A mistyped concurrency is clamped rather than opening an
        // implausible number of sessions - or none at all
        for (requested, expected) in [(0_i64, 1_usize), (10_000, MAX_CONCURRENT_WRITES)] {
            match initialise_servers(
                &[ipa1.clone(), ipa2.clone()],
                "https://ipa2.example.com",
                Some(60),
                Some(requested),
                "admin",
                &password,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => unreachable!("initialise_servers: {:?}", e),
            }

            assert_eq!(WRITE_CONFIG.lock().await.concurrent_writes, expected);

            match write_slots_for(&ipa2).await {
                Ok(slots) => assert_eq!(slots.len(), expected),
                Err(e) => unreachable!("write_slots_for: {:?}", e),
            }
        }

        // back to the defaults for the rest of this test
        match init(
            vec![ipa1.clone(), ipa2.clone()],
            "https://ipa2.example.com",
            Some(60),
        )
        .await
        {
            Ok(_) => {}
            Err(e) => unreachable!("initialise_servers: {:?}", e),
        }

        // Writes get their own connections to whichever server is taking them,
        // so raising write concurrency does not multiply the connections that
        // reads share. They are created on first use and then kept, so a
        // server that takes the writes back reuses its sessions.
        let write_slots = match write_slots_for(&ipa2).await {
            Ok(slots) => slots,
            Err(e) => unreachable!("write_slots_for: {:?}", e),
        };

        assert_eq!(write_slots.len(), DEFAULT_CONCURRENT_WRITES);
        assert!(write_slots.iter().all(|slot| slot.url == ipa2));

        // ...and they are not the slots that reads use
        let read_slots: Vec<PoolEntry> = FREEIPA_SERVERS.lock().await.clone();

        for write_slot in &write_slots {
            assert!(!read_slots
                .iter()
                .any(|read_slot| Arc::ptr_eq(&read_slot.server, &write_slot.server)));

            // Health is one record per server whatever kind of slot reports
            // it: a server that has stopped answering has stopped answering,
            // and failover compares servers with each other.
            match read_slots.iter().find(|read_slot| read_slot.url == ipa2) {
                Some(read_slot) => {
                    assert!(Arc::ptr_eq(&read_slot.health, &write_slot.health))
                }
                None => unreachable!("the write server has no read slot"),
            }
        }

        match write_slots_for(&ipa2).await {
            Ok(again) => assert!(again
                .iter()
                .zip(write_slots.iter())
                .all(|(a, b)| Arc::ptr_eq(&a.server, &b.server))),
            Err(e) => unreachable!("write_slots_for: {:?}", e),
        }

        let pool: Vec<PoolEntry> = FREEIPA_SERVERS.lock().await.clone();

        // While it is up, every write is aimed at it
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa2
        ));

        let health = match pool.iter().find(|entry| entry.url == ipa2) {
            Some(entry) => entry.health.clone(),
            None => unreachable!("the write server is not in the pool"),
        };

        // A server that is listening but never answering has to be caught too,
        // or it would absorb every write for ever. One timeout is not enough -
        // it is indistinguishable from a write that landed and whose response
        // was lost - but a run of them is.
        for _ in 1..MAX_CONSECUTIVE_TIMEOUTS {
            health.mark_timeout(&ipa2);
            assert!(matches!(
                resolve_write_target(&pool).await,
                Target::Named(ref url) if *url == ipa2
            ));
            assert!(health.down_for().is_none());
        }

        health.mark_timeout(&ipa2);
        assert!(health.down_for().is_some());

        // and any answered call clears it again
        health.mark_up(&ipa2);
        assert!(health.down_for().is_none());
        assert_eq!(
            health
                .consecutive_timeouts
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        // Confirmed down, but only just: keep aiming writes at it. Failing the
        // call is recoverable, whereas adding the same DN on a second master
        // is not.
        health.mark_down(&ipa2);
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa2
        ));

        // Down for longer than the replication window: one replacement is
        // elected - writes move to a single other master, they do not spread
        // over whatever is left.
        health.down_since.store(
            Utc::now().timestamp() - 61,
            std::sync::atomic::Ordering::SeqCst,
        );
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa1
        ));

        // If there is nothing fit to stand in, keep aiming at the configured
        // write server so the call fails cleanly rather than writing to a
        // server that may not have caught up.
        let other = match pool.iter().find(|entry| entry.url == ipa1) {
            Some(entry) => entry.health.clone(),
            None => unreachable!("the other server is not in the pool"),
        };

        other.mark_down(&ipa1);
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa2
        ));
        other.mark_up(&ipa1);

        // A server that has only just come back has not necessarily caught up
        // with what stood in for it, so it does not immediately take writes
        // again - in either direction. ipa1 is back but too recently, so
        // writes stay with the configured server...
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa2
        ));

        // ...and once ipa1 has been up long enough it can stand in again
        other.up_since.store(
            Utc::now().timestamp() - 61,
            std::sync::atomic::Ordering::SeqCst,
        );
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa1
        ));

        // When the write server itself answers again, writes stay on the
        // stand-in until it has been up for a full replication window
        health.mark_up(&ipa2);
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa1
        ));

        health.up_since.store(
            Utc::now().timestamp() - 61,
            std::sync::atomic::Ordering::SeqCst,
        );
        assert!(matches!(
            resolve_write_target(&pool).await,
            Target::Named(ref url) if *url == ipa2
        ));
    }

    #[test]
    fn test_internal_portals_map_to_bare_group_names() {
        // These three portal names map to a *bare* group name rather than the
        // usual `{portal}.{project}`, so `bob.docker.system` resolves to the
        // host's real `docker` group. That is why `force_get_user`/`get_users`
        // refuse them when they arrive from a peer, and why
        // `op-localaccount` had to be brought into line - see
        // docs/specifications/security-review-2.md (finding R13).
        let projectid = |s: &str| {
            let project = match ProjectIdentifier::parse(s) {
                Ok(p) => p,
                Err(e) => unreachable!("project {:?}: {:?}", s, e),
            };
            match identifier_to_projectid(&project, false) {
                Ok(id) => id,
                Err(e) => unreachable!("projectid {:?}: {:?}", s, e),
            }
        };

        for portal in ["openportal", "system", "instance"] {
            assert!(is_internal_portal(portal));
            assert_eq!(projectid(&format!("docker.{}", portal)), "docker");
        }

        // Anything else is namespaced by its portal, so it can never collide
        // with a pre-existing host group.
        assert!(!is_internal_portal("brics"));
        assert!(!is_internal_portal("Système"));
        assert!(!is_internal_portal(""));
        assert_eq!(projectid("proj.brics"), "brics.proj");
    }

    #[test]
    fn test_legacy_project_ids_keep_the_group_prefix() {
        let project = match ProjectIdentifier::parse("proj.brics") {
            Ok(p) => p,
            Err(e) => unreachable!("project: {:?}", e),
        };

        assert_eq!(
            match identifier_to_projectid(&project, true) {
                Ok(id) => id,
                Err(e) => unreachable!("legacy: {:?}", e),
            },
            "group.proj"
        );
    }

    #[test]
    fn test_group_member_uids_come_from_member_user() {
        // FreeIPA's JSON-RPC returns group members as `member_user` (with the
        // underscore), not `memberuser`. Reading the wrong key silently yields
        // an empty set, which makes every membership test succeed vacuously -
        // so pin the key name.
        let group = serde_json::json!({
            "cn": ["brics.proj"],
            "member_user": ["alice", "bob"],
        });

        let uids = raw_group_member_uids(&group);
        assert_eq!(uids.len(), 2);
        assert!(uids.contains("alice"));
        assert!(uids.contains("bob"));

        // Absent, wrong-typed and non-string entries degrade to "no members"
        // rather than panicking on a hostile or unexpected response.
        assert!(raw_group_member_uids(&serde_json::json!({})).is_empty());
        assert!(raw_group_member_uids(&serde_json::json!({"memberuser": ["alice"]})).is_empty());
        assert!(raw_group_member_uids(&serde_json::json!({"member_user": "alice"})).is_empty());
        assert_eq!(
            raw_group_member_uids(&serde_json::json!({"member_user": ["alice", 42, null]})).len(),
            1
        );
    }
}
