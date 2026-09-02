// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.IOException;
import java.net.URI;
import java.net.http.HttpClient;
import java.net.http.HttpRequest;
import java.net.http.HttpResponse;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.UUID;

/**
 * A client for the OpenPortal bridge API.
 *
 * <p>This is the Java equivalent of the {@code openportal} Python module, and it
 * exists for the same reason: a portal has to talk to a running {@code op-bridge}
 * over localhost HTTP, and every call has to be signed. The method names match
 * the Python module's functions, so the two READMEs describe the same thing.
 *
 * <pre>{@code
 * BridgeClient bridge = BridgeClient.load(Path.of("bridge.toml"));
 *
 * String me = bridge.getPortal();                  // "site"
 * bridge.syncOfferings(List.of(Destination.parse("cluster1.site.allocator")));
 *
 * for (Job job : bridge.fetchJobs()) {
 *     bridge.sendResult(handle(job));
 * }
 * }</pre>
 *
 * <p><b>The bridge is not internet-facing.</b> It binds to localhost by default
 * and the invite file is a credential; put neither on a public interface.
 *
 * <p>Instances are immutable and the underlying {@link HttpClient} is thread-safe,
 * so one client can be shared. Each call signs itself with a fresh date and
 * nonce, which is why nothing here is cached.
 */
public final class BridgeClient {

    /**
     * How long to wait for the bridge.
     *
     * <p>Well inside the thirty seconds a caller waits for an answer (§3.4): a
     * request to the bridge that is going to hang should fail while there is
     * still time to do something about it.
     */
    private static final Duration TIMEOUT = Duration.ofSeconds(10);

    private final BridgeConfig config;
    private final HttpClient http;

    public BridgeClient(BridgeConfig config) {
        this(
                config,
                HttpClient.newBuilder()
                        .connectTimeout(TIMEOUT)
                        // The bridge is on localhost, so a proxy configured for
                        // the outside world must not be consulted for it.
                        .proxy(HttpClient.Builder.NO_PROXY)
                        .build());
    }

    public BridgeClient(BridgeConfig config, HttpClient http) {
        this.config = config;
        this.http = http;
    }

    /** Load the bridge invite file written by {@code op-bridge bridge --config}. */
    public static BridgeClient load(Path inviteFile) throws IOException {
        return new BridgeClient(BridgeConfig.load(inviteFile));
    }

    // ----------------------------------------------------------------------
    // Who we are, and what we offer
    // ----------------------------------------------------------------------

    /** This portal's own agent name - the middle element of every offering. */
    public String getPortal() {
        return get("get_portal").asText();
    }

    /** The offerings currently registered with OpenPortal. */
    public List<Destination> getOfferings() {
        return destinations(get("get_offerings"));
    }

    /** As {@link #getOfferings}, keeping the wire type. */
    public Destinations offerings() {
        return Destinations.of(getOfferings());
    }

    /**
     * Register the complete set of offerings, replacing whatever was there.
     *
     * <p>A replace and not a merge: anything absent is withdrawn, and an empty
     * list withdraws everything. Until an offering is registered, requests for it
     * have nowhere to land and are <b>held</b> rather than refused (§1.1) - which
     * is the least helpful failure in the system, so call this at startup and
     * again whenever the set changes.
     */
    public List<Destination> syncOfferings(List<Destination> offerings) {
        return destinations(post("sync_offerings", offeringsBody(offerings)));
    }

    public List<Destination> addOfferings(List<Destination> offerings) {
        return destinations(post("add_offerings", offeringsBody(offerings)));
    }

    public List<Destination> removeOfferings(List<Destination> offerings) {
        return destinations(post("remove_offerings", offeringsBody(offerings)));
    }

    // ----------------------------------------------------------------------
    // The bridge board: jobs OpenPortal wants this portal to answer
    // ----------------------------------------------------------------------

    /**
     * Fetch one job by id, as the {@code signal_url} named it.
     *
     * <p>The id is the credential: it is a random UUID known only to the bridge
     * and to you, so an id the bridge does not have is not a request to act on.
     * Its own signal endpoint should treat an unknown id as a {@code 403} rather
     * than fetching it (§3.1).
     */
    public Job fetchJob(UUID id) {
        return Job.of(post("fetch_job", Json.write(id.toString())));
    }

    /**
     * Every job outstanding for this portal.
     *
     * <p>The safety net behind {@code signal_url}: a signal can be lost, and
     * without a slower sweep that job waits until it expires.
     */
    public List<Job> fetchJobs() {
        JsonNode node = get("fetch_jobs");
        List<Job> jobs = new ArrayList<>();

        if (node.isArray()) {
            node.forEach(job -> jobs.add(Job.of(job)));
        }

        return jobs;
    }

    /**
     * Post an answered job back.
     *
     * <p>The bridge matches it to the board by {@code id}, so the id must be
     * unchanged and the version higher than the one fetched - which is what
     * {@link Job#completed} and {@link Job#errored} take care of. Retry a failed
     * post: the reference implementation tries five times at one-second intervals
     * before giving up.
     */
    public void sendResult(Job job) {
        post("send_result", Json.write(job.json()));
    }

    /**
     * Fetch a notification by id, as the {@code notification_url} named it.
     *
     * <p>Fails if the id is unknown - a notification is removed once fetched, so
     * fetching the same one twice is an error rather than a repeat.
     */
    public Notification fetchNotification(UUID id) {
        return Notification.fromJson(post("fetch_notification", Json.write(id.toString())));
    }

    // ----------------------------------------------------------------------
    // Asking OpenPortal to do something
    // ----------------------------------------------------------------------

    /**
     * Submit a command, returning the job it created.
     *
     * <p>The job comes back immediately and unfinished; poll it with
     * {@link #status} until it is. This is the direction an <i>awarding</i> portal
     * drives - a site portal mostly answers rather than asks.
     */
    public Job run(String command) {
        ObjectNode body = Json.object();
        body.put("command", command);

        return Job.of(post("run", Json.write(body)));
    }

    /**
     * Re-read a job from the bridge, to see whether it has finished.
     *
     * <p>Only its id is sent - the bridge holds the job, and posting a whole job
     * here is {@link #sendResult}, which is a different thing entirely.
     */
    public Job status(Job job) {
        return get(job.id());
    }

    /**
     * Re-read a job by id.
     *
     * <p>{@code status} wants the id wrapped in an object - {@code {"job": "…"}} -
     * where {@code fetch_job} wants the bare string. They are not interchangeable,
     * and the wrong one is a {@code 500}.
     */
    public Job get(UUID id) {
        ObjectNode body = Json.object();
        body.put("job", id.toString());

        return Job.of(post("status", Json.write(body)));
    }

    /**
     * Wait for a job to finish, polling until it does.
     *
     * <p>Convenience for a caller driving OpenPortal rather than answering it.
     * The timeout should be well under the two-minute job expiry, and a job that
     * has expired is finished as far as this is concerned.
     */
    public Job waitFor(Job job, Duration timeout) {
        long deadline = System.nanoTime() + timeout.toNanos();
        Job latest = job;

        while (!latest.isFinished() && System.nanoTime() < deadline) {
            if (latest.isExpired()) {
                break;
            }

            try {
                Thread.sleep(500);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                break;
            }

            latest = status(latest);
        }

        return latest;
    }

    /** Send a fire-and-forget notification. Nothing is returned and nothing is acknowledged. */
    public void notify(String command) {
        ObjectNode body = Json.object();
        body.put("command", command);

        post("notify", Json.write(body));
    }

    /**
     * The health of the bridge and, nested under it, the agents behind it.
     *
     * <p>The one call that needs no arguments and answers even when the network
     * behind the bridge is down - which is what makes it the first thing to try
     * when something else is failing. A {@code 401} here is a key or clock
     * problem, not a network one.
     */
    public Health health() {
        return Health.fromJson(get("health"));
    }

    /**
     * An agent's diagnostics report: what failed, what was slow, what expired.
     *
     * <p>{@code destination} is the dotted path to the agent, and an
     * <b>empty string</b> means the bridge itself rather than "all of them".
     */
    public Diagnostics diagnostics(String destination) {
        ObjectNode body = Json.object();
        body.put("destination", destination == null ? "" : destination);

        return Diagnostics.fromJson(post("diagnostics", Json.write(body)));
    }

    /**
     * Restart an agent.
     *
     * <p>{@code restartType} is the agent's own vocabulary ({@code "soft"},
     * {@code "hard"}); {@code destination} is the dotted path, and an empty
     * string is the bridge itself. The answer says the restart was accepted, not
     * that the agent is back - it will be unreachable for a moment either way.
     */
    public RestartResponse restart(String restartType, String destination) {
        ObjectNode body = Json.object();
        body.put("restart_type", restartType);
        body.put("destination", destination == null ? "" : destination);

        return RestartResponse.fromJson(post("restart", Json.write(body)));
    }

    // ----------------------------------------------------------------------
    // The HTTP, and the signing
    // ----------------------------------------------------------------------

    /**
     * The body for the three offering endpoints: a JSON array of destination
     * strings.
     *
     * <p>An array, not the bracketed {@code Destinations} text form - that one
     * belongs inside instruction strings, and sending it here is a {@code 500}.
     * See {@link Destinations}, which has both.
     */
    private static String offeringsBody(List<Destination> offerings) {
        return Json.write(Destinations.of(offerings).toJson());
    }

    /** The reply from those endpoints, which is an array of the same. */
    private static List<Destination> destinations(JsonNode node) {
        List<Destination> parsed = new ArrayList<>();

        if (node.isArray()) {
            node.forEach(entry -> parsed.add(Destination.parse(entry.asText())));
        }

        return parsed;
    }

    private JsonNode get(String function) {
        return call("get", function, "");
    }

    private JsonNode post(String function, String body) {
        return call("post", function, body);
    }

    /**
     * Sign and send one call.
     *
     * <p>The date and the body are signed, so both have to reach the wire exactly
     * as they were signed: the {@code Date} header is the same string that went
     * into the signature, and the body is written as the bytes that were signed
     * rather than re-serialised. A JSON library that reformats a body after
     * signing produces a {@code 401} that looks like a key problem.
     */
    private JsonNode call(String protocol, String function, String body) {
        String date = BridgeAuth.now();
        String nonce = BridgeAuth.nonce();
        byte[] payload = body.getBytes(StandardCharsets.UTF_8);

        HttpRequest.Builder request =
                HttpRequest.newBuilder(URI.create(config.url() + "/" + function))
                        .timeout(TIMEOUT)
                        .header(
                                "Authorization",
                                BridgeAuth.authorization(
                                        config.key(), protocol, date, function, body, nonce))
                        .header("Date", date)
                        .header("Content-Type", "application/json")
                        .header("Accept", "application/json")
                        .header("X-Nonce", nonce)
                        // Without this header the bridge verifies the older,
                        // ambiguous version 1 form and rejects a version 2
                        // signature - so it is not optional for this client.
                        .header("X-OpenPortal-Signature-Version", "2");

        request =
                body.isEmpty()
                        ? request.GET()
                        : request.POST(HttpRequest.BodyPublishers.ofByteArray(payload));

        HttpResponse<String> response;

        try {
            response = http.send(request.build(), HttpResponse.BodyHandlers.ofString());
        } catch (IOException e) {
            throw new OpenPortalOtherError("could not reach the bridge at " + config.url() + ": " + e);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new OpenPortalOtherError("interrupted while calling the bridge");
        }

        return body(function, response);
    }

    /**
     * The response body, or the error it describes.
     *
     * <p>A {@code 401} is worth its own message: it means the signature did not
     * verify, and the two usual causes are a clock more than five seconds out and
     * a body that changed after it was signed - neither of which is a wrong key,
     * which is what a bare "Unauthorized" suggests.
     */
    private JsonNode body(String function, HttpResponse<String> response) {
        int status = response.statusCode();
        String text = response.body() == null ? "" : response.body();

        if (status == 200) {
            return text.isBlank() ? Json.object() : Json.parse(text);
        }

        String detail = text.isBlank() ? "" : ": " + message(text);

        if (status == 401) {
            throw new OpenPortalOtherError(
                    "the bridge rejected the signature for '"
                            + function
                            + "'"
                            + detail
                            + " - check the key, that this clock is within five seconds of the"
                            + " bridge's, and that the body was not altered after signing");
        }

        throw new OpenPortalOtherError("the bridge answered " + status + " for '" + function + "'" + detail);
    }

    /** The {@code message} field of an error response, or the raw text. */
    private static String message(String text) {
        try {
            JsonNode node = Json.parse(text);

            return node.has("message") ? node.get("message").asText() : text;
        } catch (RuntimeException e) {
            return text;
        }
    }

    /** Where this client is pointed, for a log line. */
    public String url() {
        return config.url();
    }

    /** The invite file's own view of itself, for a caller that wants to re-read it. */
    public Optional<String> describe() {
        return Optional.of("bridge at " + config.url());
    }
}
