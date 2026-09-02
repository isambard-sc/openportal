// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.Instant;
import java.util.Optional;
import java.util.UUID;

/**
 * One job from the bridge board: a request to this portal, and the answer to it.
 *
 * <p><b>The raw JSON is kept, and answers are built from a copy of it.</b> A job
 * carries fields this client has no opinion about - {@code board},
 * {@code domain}, {@code domain_version}, and whatever a later version adds -
 * and the bridge matches a posted result to the board by {@code id} and
 * {@code version}. Rebuilding a job from typed fields would quietly drop the
 * rest, so {@link #completed} and {@link #errored} deep-copy what arrived and
 * change only what they must. Nothing is lost by a client that does not
 * understand it.
 *
 * <p>The two ways to answer are {@link #completed} and {@link #errored}, and for
 * half the contract the error <i>is</i> the answer - see {@link OpenPortalError}.
 * The one thing never to do is neither: silence becomes a timeout for whoever is
 * waiting.
 */
public final class Job {

    /**
     * What a version is bumped by when a job is answered.
     *
     * <p>A thousand rather than one, matching the Rust side: the board takes the
     * highest version it has seen, and leaving room means an answer cannot be
     * overtaken by an intermediate update that was in flight.
     */
    private static final long VERSION_STRIDE = 1000;

    private final ObjectNode raw;

    private Job(ObjectNode raw) {
        this.raw = raw;
    }

    /** Wrap a job as the bridge sent it. */
    public static Job of(JsonNode node) {
        if (node == null || !node.isObject()) {
            throw new IllegalArgumentException("a job is a JSON object");
        }

        return new Job((ObjectNode) node.deepCopy());
    }

    /** Parse a job from the JSON text the bridge returned. */
    public static Job parse(String json) {
        return of(Json.parse(json));
    }

    /** The job exactly as it will be posted back. */
    public JsonNode json() {
        return raw;
    }

    @Override
    public String toString() {
        return Json.write(raw);
    }

    // ----------------------------------------------------------------------
    // What arrived
    // ----------------------------------------------------------------------

    public UUID id() {
        return UUID.fromString(text("id"));
    }

    public Instant created() {
        return Instant.ofEpochSecond(raw.path("created").asLong());
    }

    public Instant changed() {
        return Instant.ofEpochSecond(raw.path("changed").asLong());
    }

    /**
     * When this job stops being answerable.
     *
     * <p>Two minutes after it was created - but the caller gives up long before
     * that, so budget thirty seconds and serve slow answers from cache
     * (§3.4). A result posted after this is accepted by the bridge and
     * discarded.
     */
    public Instant expires() {
        return Instant.ofEpochSecond(raw.path("expires").asLong());
    }

    public boolean isExpired() {
        return Instant.now().isAfter(expires());
    }

    public long version() {
        return raw.path("version").asLong();
    }

    /** The whole command: destination, then instruction. */
    public String command() {
        return text("command");
    }

    /** The route this job took, ending in the agent that is to act on it. */
    public Destination destination() {
        String command = command();
        int space = command.indexOf(' ');

        return Destination.parse(space < 0 ? command : command.substring(0, space));
    }

    /** The verb and arguments. */
    public Instruction instruction() {
        String command = command();
        int space = command.indexOf(' ');

        return Instruction.parse(space < 0 ? "" : command.substring(space + 1));
    }

    /**
     * Where the request came from, when it came from another portal.
     *
     * <p>Set by your own portal agent and never by the caller, which is why it is
     * the field to authorise against (§1.2). Its first element is the portal that
     * asked; its last is the offering they came in through.
     */
    public Optional<Destination> forwardedFor() {
        JsonNode node = raw.get("forwarded_for");

        return node == null || node.isNull()
                ? Optional.empty()
                : Optional.of(Destination.parse(node.asText()));
    }

    public Status state() {
        return Status.parse(text("state"));
    }

    public boolean isFinished() {
        return state().isFinished();
    }

    public boolean isError() {
        return state() == Status.ERROR;
    }

    /** The {@code result_type} of the answer, if this job has one. */
    public Optional<String> resultType() {
        JsonNode node = raw.get("result_type");

        return node == null || node.isNull() ? Optional.empty() : Optional.of(node.asText());
    }

    /**
     * The result as it sits on the wire: a JSON string that itself contains JSON.
     *
     * <p>That double encoding is deliberate and it is not a bug to work around -
     * a {@code ProjectMapping} arrives as {@code "\"a:b\""}. {@link #result()}
     * unwraps one layer for you.
     */
    public Optional<String> resultText() {
        JsonNode node = raw.get("result");

        return node == null || node.isNull() ? Optional.empty() : Optional.of(node.asText());
    }

    /** The result with the outer layer of encoding removed. */
    public Optional<JsonNode> result() {
        return resultText().map(Json::parse);
    }

    /**
     * The result, read by whichever type's reader you expect.
     *
     * <p>{@code job.result(ProjectMapping::parse)} for a mapping,
     * {@code job.result(AwardDetails::fromJson)} for an award. Empty when the
     * job has no result - so a completed job that answered {@code None} and one
     * that has not answered yet look the same here; ask {@link #state} to tell
     * them apart.
     *
     * <p>Does not check {@link #resultType} against the reader. If you care
     * that the answer is the type you asked for - and for a job you dispatched
     * on you should - compare it yourself.
     */
    public <T> Optional<T> result(java.util.function.Function<JsonNode, T> reader) {
        return result().map(reader);
    }

    /**
     * The result as text, read by a string parser.
     *
     * <p>For the types whose whole wire form is a string:
     * {@code job.resultText(ProjectMapping::parse)}. Note this reads through
     * {@link #result()} and takes the text <i>inside</i> the JSON string, so
     * the parser sees {@code a:b} rather than {@code "a:b"} - which is what
     * {@link #resultText()} would have handed it, and why that one is not the
     * accessor to build a typed value from.
     */
    public <T> Optional<T> resultText(java.util.function.Function<String, T> parser) {
        return result().map(JsonNode::asText).map(parser);
    }

    /**
     * The typed error this job failed with, or empty if it did not fail.
     *
     * <p>Taken from {@code error.kind} when the job carries one - that was decided
     * by the agent that failed, so nothing here reads prose - and recovered from
     * the message otherwise.
     */
    public Optional<OpenPortalError> error() {
        if (!isError()) {
            return Optional.empty();
        }

        JsonNode error = raw.get("error");
        String message = resultText().orElse("");

        if (error != null && error.isObject()) {
            return Optional.of(
                    OpenPortalError.fromKind(
                            error.path("kind").asText(""), error.path("message").asText(message)));
        }

        return Optional.of(OpenPortalError.decode(message));
    }

    /** The machine-readable kind of this job's failure, or {@code ""}. */
    /**
     * The failure text, or {@code ""} on a job that did not fail.
     *
     * <p>The wire-encoded form - {@code "ManagedProjectPendingError: awaiting
     * approval"}, with the class prefix still on it. {@link #error} decodes it
     * into a typed exception, which is what to branch on; this is for showing a
     * human.
     */
    public String errorMessage() {
        return isError() ? resultText().orElse("") : "";
    }

    /**
     * A word for how far this job has got, for a progress display.
     *
     * <p>Its state's name, except for a running job that has posted interim
     * text, whose text this is instead. Never empty - a job always has some
     * state - so this is not the field to test for a failure; use
     * {@link #isError}.
     */
    public String progressMessage() {
        if (state() == Status.RUNNING) {
            return resultText().filter(text -> !text.isBlank()).orElse("Running");
        }

        // A duplicate reports as pending: it is waiting on the job it duplicates.
        return state() == Status.DUPLICATE ? Status.PENDING.wire() : state().wire();
    }

    public String errorKind() {
        JsonNode error = raw.get("error");

        return error == null || !error.isObject() ? "" : error.path("kind").asText("");
    }

    // ----------------------------------------------------------------------
    // Answering
    // ----------------------------------------------------------------------

    /** Answer with a typed result. */
    public Job completed(OpenPortalType value) {
        ObjectNode next = answering(Status.COMPLETE);

        // `result` is the JSON of the value, held *as a string* - so the value's
        // JSON is encoded once here and once more when the job is serialised.
        next.put("result", Json.write(value.toJson()));
        next.put("result_type", value.typeName());
        next.remove("error");

        return new Job(next);
    }

    /** Answer with a result whose type name you are supplying yourself. */
    public Job completed(JsonNode value, String typeName) {
        ObjectNode next = answering(Status.COMPLETE);
        next.put("result", Json.write(value));
        next.put("result_type", typeName);
        next.remove("error");

        return new Job(next);
    }

    /** Answer with no result - a completed job that returns nothing. */
    public Job completedNone() {
        ObjectNode next = answering(Status.COMPLETE);
        next.putNull("result");
        next.put("result_type", "None");
        next.remove("error");

        return new Job(next);
    }

    /**
     * Answer with a failure.
     *
     * <p>The message goes to {@code result} exactly where failure text has always
     * lived, and the structured {@code error} beside it carries the kind, so a
     * reader of either sees the same thing.
     */
    public Job errored(OpenPortalError error) {
        return errored(error.encode());
    }

    /** As {@link #errored(OpenPortalError)}, for a message in the wire form. */
    public Job errored(String message) {
        ObjectNode next = answering(Status.ERROR);
        String encoded = message == null ? "" : message;

        next.put("result", encoded);
        next.put("result_type", "Error");

        ObjectNode error = Json.object();
        error.put("kind", JobErrorKind.classify(encoded));
        error.put("message", encoded);
        next.set("error", error);

        return new Job(next);
    }

    /**
     * A copy of this job, stamped as changed and answered.
     *
     * <p>Refuses a job whose state cannot be answered, matching the states the
     * Rust side accepts: a completion needs {@code Pending} or {@code Running},
     * and a failure also accepts {@code Duplicate}. Two of the refusals matter
     * for different reasons - answering an already-finished job would overwrite
     * the first answer rather than being ignored, because the board takes the
     * higher version; and a {@code Created} job has not been handed out yet, so
     * an answer to it is an answer to a question nobody asked.
     */
    private ObjectNode answering(Status state) {
        boolean answerable = this.state() == Status.PENDING
                || this.state() == Status.RUNNING
                || (state == Status.ERROR && this.state() == Status.DUPLICATE);

        if (!answerable) {
            throw new IllegalStateException("cannot set " + state.wire().toLowerCase(
                    java.util.Locale.ROOT) + " on job " + id() + " in state " + state().wire());
        }

        ObjectNode next = raw.deepCopy();
        next.put("changed", Instant.now().getEpochSecond());
        next.put("version", version() + VERSION_STRIDE);
        next.put("state", state.wire());

        return next;
    }

    private String text(String field) {
        JsonNode node = raw.get(field);

        if (node == null || node.isNull()) {
            throw new IllegalStateException("job has no " + field);
        }

        return node.asText();
    }
}
