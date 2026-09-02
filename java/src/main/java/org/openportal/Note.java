// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import java.time.Instant;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;

/**
 * A timestamped message attached to an award.
 *
 * <p>Append-only: the awarding portal adds notes, and a site reading an award
 * should treat the list as a log rather than something to edit. Typically how
 * an awarder explains a decision to the project team.
 */
public record Note(Instant timestamp, String author, String text) {

    private static final DateTimeFormatter DISPLAY =
            DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm").withZone(ZoneOffset.UTC);

    public Note {
        if (timestamp == null) {
            throw new IllegalArgumentException("a note needs a timestamp");
        }

        author = author == null ? "" : author;
        text = text == null ? "" : text;
    }

    /** A note timestamped now. */
    public static Note of(String author, String text) {
        return new Note(Instant.now(), author, text);
    }

    public static Note at(Instant timestamp, String author, String text) {
        return new Note(timestamp, author, text);
    }

    public JsonNode toJson() {
        return Json.object()
                .put("timestamp", Times.toJson(timestamp))
                .put("author", author)
                .put("text", text);
    }

    public static Note fromJson(JsonNode node) {
        return new Note(
                Times.fromJson(node.path("timestamp")),
                node.path("author").asText(""),
                node.path("text").asText(""));
    }

    /** {@code "[2026-01-02 03:04 UTC — alice] the text"}, as the Rust side prints it. */
    @Override
    public String toString() {
        return "[" + DISPLAY.format(timestamp) + " UTC — " + author + "] " + text;
    }
}
