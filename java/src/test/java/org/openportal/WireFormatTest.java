// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.util.List;
import org.junit.jupiter.api.DisplayName;
import org.junit.jupiter.api.Test;

/** The shapes on the wire: destinations, instructions, errors, and answering a job. */
class WireFormatTest {

    private static final String JOB =
            """
            {"id":"a1b2c3d4-e5f6-7890-abcd-ef1234567890",
             "created":1700000000,"changed":1700000005,"expires":4000000000,"version":2,
             "command":"site.bridge.cluster1 create_project myaward1.allocator {\\"name\\":\\"A B\\"}",
             "state":"Pending","result":null,"result_type":null,
             "forwarded_for":"allocator.site.cluster1",
             "board":{"name":"cluster1","zone":"default"},"domain":"greatwestern"}
            """;

    @Test
    @DisplayName("a destination is read either way round")
    void destinations() {
        Destination job = Destination.parse("site.bridge.cluster1");

        assertEquals("site", job.first());
        assertEquals("cluster1", job.last());
        assertEquals("cluster1.bridge.site", job.reverse().toString());
    }

    @Test
    @DisplayName("Destinations has an array form for JSON and a bracketed one for text")
    void destinationsSet() {
        Destinations one = Destinations.parse("cluster1.site.allocator");
        Destinations two = Destinations.parse("[cluster1.site.allocator, cluster2.site.allocator]");

        // A single destination has no brackets, which is what the bridge produces.
        assertEquals("cluster1.site.allocator", one.toString());
        assertEquals("[cluster1.site.allocator, cluster2.site.allocator]", two.toString());
        assertEquals("[]", Destinations.parse("[]").toString());

        // ...but over HTTP both are arrays.
        assertEquals("[\"cluster1.site.allocator\"]", Json.write(one.toJson()));
        assertEquals(2, two.destinations().size());
    }

    @Test
    @DisplayName("an instruction's JSON argument keeps its spaces")
    void instructionArguments() {
        Instruction instruction =
                Instruction.parse("create_project myaward1.allocator {\"name\":\"My First Award\"}");

        assertEquals("create_project", instruction.command());
        assertEquals(
                List.of("myaward1.allocator", "{\"name\":\"My First Award\"}"),
                instruction.arguments());
        assertEquals("", instruction.argument(7));
    }

    @Test
    @DisplayName("an error round-trips through its wire form")
    void errorRoundTrip() {
        OpenPortalError pending = new ManagedProjectPendingError("awaiting approval");

        assertEquals("ManagedProjectPendingError: awaiting approval", pending.encode());

        // ...including through the wrapper the portal agent adds.
        OpenPortalError decoded =
                OpenPortalError.decode("RuntimeError{ManagedProjectPendingError: awaiting approval}");

        assertInstanceOf(ManagedProjectPendingError.class, decoded);
        assertEquals("awaiting approval", decoded.getMessage());
    }

    @Test
    @DisplayName("an unrecognised message keeps its text")
    void unknownError() {
        OpenPortalError decoded = OpenPortalError.decode("the disk caught fire");

        assertInstanceOf(OpenPortalOtherError.class, decoded);
        assertEquals("the disk caught fire", decoded.getMessage());
    }

    @Test
    @DisplayName("a class name that merely opens the text is not a class")
    void classNamePrefixIsNotAClassification() {
        assertInstanceOf(
                OpenPortalOtherError.class,
                OpenPortalError.decode("ManagedProjectPendingErrorish nonsense"));
        assertEquals(JobErrorKind.UNKNOWN, JobErrorKind.classify("ManagedProjectPendingErrorish"));
    }

    @Test
    @DisplayName("a kind beats the prose it travels with")
    void kindWins() {
        assertInstanceOf(
                ManagedProjectRejectedError.class,
                OpenPortalError.fromKind("award_rejected", "no capacity this quarter"));
        assertEquals(JobErrorKind.AWARD_PENDING,
                JobErrorKind.classify("ManagedProjectPendingError: later"));
    }

    @Test
    @DisplayName("a job is read without losing what it does not understand")
    void jobIsParsed() {
        Job job = Job.parse(JOB);

        assertEquals("cluster1", job.destination().last());
        assertEquals("allocator.site.cluster1", job.forwardedFor().orElseThrow().toString());
        assertEquals("create_project", job.instruction().command());
        assertEquals(Status.PENDING, job.state());
        assertEquals(false, job.isFinished());
    }

    @Test
    @DisplayName("answering keeps every field the client has no opinion about")
    void answeringPreservesUnknownFields() {
        Job answered = Job.parse(JOB).completed(Json.text("myaward1.allocator:myproject1.site"),
                                                "ProjectMapping");

        assertEquals("Complete", answered.json().get("state").asText());
        assertEquals(1002, answered.version());
        assertEquals("ProjectMapping", answered.resultType().orElseThrow());

        // The result is a JSON string containing JSON - one layer to unwrap.
        assertEquals("\"myaward1.allocator:myproject1.site\"", answered.resultText().orElseThrow());
        assertEquals("myaward1.allocator:myproject1.site", answered.result().orElseThrow().asText());

        // ...and the fields this client never looked at are still there.
        assertEquals("greatwestern", answered.json().get("domain").asText());
        assertEquals("cluster1", answered.json().get("board").get("name").asText());
    }

    @Test
    @DisplayName("erroring a job sets the kind beside the message")
    void erroringSetsKind() {
        Job answered = Job.parse(JOB).errored(new ManagedProjectPendingError("awaiting approval"));

        assertEquals("Error", answered.json().get("state").asText());
        assertEquals("Error", answered.resultType().orElseThrow());
        assertEquals("award_pending", answered.errorKind());
        assertEquals(
                "ManagedProjectPendingError: awaiting approval",
                answered.resultText().orElseThrow());
        assertInstanceOf(ManagedProjectPendingError.class, answered.error().orElseThrow());
    }

    @Test
    @DisplayName("only a job in an answerable state can be answered")
    void answerableStates() {
        // Answering twice would not be ignored - the board takes the higher
        // version, so the second answer would overwrite the first.
        Job answered = Job.parse(JOB).completedNone();

        assertTrue(
                assertInstanceOf(
                                IllegalStateException.class,
                                org.junit.jupiter.api.Assertions.assertThrows(
                                        IllegalStateException.class, answered::completedNone))
                        .getMessage()
                        .contains("in state Complete"));

        // And a `Created` job has not been handed out yet, so an answer to it
        // is an answer to a question nobody asked. These are the states the
        // Rust side accepts, and getting them wrong here means a result the
        // board refuses.
        Job created = Job.parse(JOB.replace("\"state\":\"Pending\"", "\"state\":\"Created\""));

        org.junit.jupiter.api.Assertions.assertThrows(
                IllegalStateException.class, created::completedNone);
        org.junit.jupiter.api.Assertions.assertThrows(
                IllegalStateException.class, () -> created.errored("no"));

        // A duplicate can be errored but not completed - it has no work of its
        // own to have finished.
        Job duplicate =
                Job.parse(JOB.replace("\"state\":\"Pending\"", "\"state\":\"Duplicate\""));

        assertEquals("Error", duplicate.errored("no").json().get("state").asText());
        org.junit.jupiter.api.Assertions.assertThrows(
                IllegalStateException.class, duplicate::completedNone);
    }
}
