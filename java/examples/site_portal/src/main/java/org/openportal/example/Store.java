// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import org.openportal.Json;

/**
 * Where this portal keeps what it knows: JSON files under one directory.
 *
 * <p>A real portal has a database. What is worth copying is not the storage but
 * the <b>shape</b>, and three decisions in it:
 *
 * <ul>
 *   <li><b>An award is keyed on the offering as well as its identifier.</b>
 *       {@code myaward1.allocator} on {@code cluster1} and the same name on
 *       {@code cluster2} are two different awards for two different resources.
 *       One directory per offering makes the key structural rather than
 *       remembered.
 *   <li><b>Usage belongs to the project, not to the award.</b> A project's
 *       usage on a day is billed to whichever award it was last attached to
 *       <i>that day</i> - which is not settled until the day ends, because
 *       attaching an award this afternoon takes the whole of today. So figures
 *       are recorded against the project and awards claim days of them when a
 *       report is built.
 *   <li><b>State is read fresh on every access.</b> An operator approving an
 *       award and a handler answering a job are different requests; a cache
 *       between them goes stale.
 * </ul>
 *
 * <p>Unlike the Python example this is an object with a root directory rather
 * than a module reading an environment variable - which is what lets the tests
 * point it at a temporary directory instead of the portal's own.
 */
public final class Store {

    private final Path root;

    public Store(Path root) {
        this.root = root;
    }

    public Path root() {
        return root;
    }

    // ---- offerings ---------------------------------------------------------

    /**
     * Every resource we advertise, by name.
     *
     * <p>Empty until an operator adds one, and empty is an ordinary state
     * rather than a misconfiguration: a site that advertises nothing simply
     * cannot be asked for anything yet.
     */
    public Map<String, Offering> offerings() {
        JsonNode stored = readOrEmpty(offeringsPath());
        Map<String, Offering> offerings = new TreeMap<>();

        stored.fields().forEachRemaining(entry ->
                offerings.put(entry.getKey(), new Offering(entry.getKey(), entry.getValue())));

        return offerings;
    }

    public Optional<Offering> offering(String name) {
        return Optional.ofNullable(offerings().get(name));
    }

    /**
     * Start advertising a resource, or change its templates or conversions.
     *
     * <p>An upsert, deliberately: the operator API is retried and re-run like
     * everything else, and "add the cluster I already have" should not be an
     * error. {@code since} is kept from the first time, because that is when we
     * started offering it, and omitted {@code conversions} keep what was
     * already agreed so the templates can be changed on their own.
     */
    public Offering addOffering(
            String name, List<String> templates, Map<String, Double> conversions, LocalDate on) {
        Map<String, Offering> offerings = offerings();
        Offering existing = offerings.get(name);

        ObjectNode raw = Json.object();

        java.util.SortedSet<String> unique = new java.util.TreeSet<>(templates);
        var array = raw.putArray("templates");
        unique.forEach(array::add);

        LocalDate since = existing != null && existing.since().isPresent()
                ? existing.since().get()
                : on;
        raw.put("since", since.toString());

        Map<String, Double> agreed = conversions;

        if (agreed == null && existing != null) {
            agreed = existing.conversions();
        }

        if (agreed != null && !agreed.isEmpty()) {
            ObjectNode node = raw.putObject("conversions");
            agreed.forEach(node::put);
        }

        Offering offering = new Offering(safe(name, "offering"), raw);
        offerings.put(offering.name(), offering);
        saveOfferings(offerings);

        return offering;
    }

    /**
     * Stop advertising a resource. Returns what was removed.
     *
     * <p><b>The awards on it are kept.</b> Withdrawing an offering says what we
     * advertise <i>now</i>; it does not rewrite what happened. Those awards
     * still own the days they were attached for, and deleting them would make a
     * later usage report empty - and an empty report is vacuously complete,
     * which is how the last days of an award get silently lost.
     */
    public Optional<Offering> removeOffering(String name) {
        Map<String, Offering> offerings = offerings();
        Offering removed = offerings.remove(name);

        if (removed != null) {
            saveOfferings(offerings);
        }

        return Optional.ofNullable(removed);
    }

    private void saveOfferings(Map<String, Offering> offerings) {
        ObjectNode node = Json.object();
        offerings.forEach((name, offering) -> node.set(name, offering.json()));
        writeAtomically(offeringsPath(), node);
    }

    // ---- awards ------------------------------------------------------------

    /**
     * One award, or empty if we hold no such award <b>on that offering</b>.
     *
     * <p>Both halves of the key are required. Asking for an award on a resource
     * it was never created on is a legitimate question with the answer "no".
     */
    public Optional<Award> award(String offering, String projectId) {
        Path path = awardPath(offering, projectId);

        if (!Files.exists(path)) {
            return Optional.empty();
        }

        return Optional.of(new Award(offering, projectId, read(path)));
    }

    /** Record a brand-new award on one offering, awaiting approval. */
    public Award create(
            String offering, String projectId, JsonNode details, String forwardedFor) {
        ObjectNode raw = Json.object();
        raw.set("details", details.deepCopy());
        raw.put("state", Award.PENDING);
        raw.put("reason", "awaiting approval by a site administrator");

        // Nothing of ours is attached yet - approving is what attaches it.
        raw.putArray("attachments");

        if (forwardedFor != null) {
            raw.put("forwarded_for", forwardedFor);
        }

        Award award = new Award(offering, projectId, raw);
        save(award);

        return award;
    }

    public void save(Award award) {
        writeAtomically(awardPath(award.offering(), award.projectId()), award.json());
    }

    /**
     * Attach an approved award to one of our projects, from {@code on} onwards.
     *
     * <p>Appends to the history rather than overwriting it, so an award
     * re-attached after a gap keeps the days it owned before, and one moved
     * between projects keeps the days it owned on the first.
     *
     * <p>Any open attachment is closed as of {@code on} - <b>not</b> the day
     * before. Both ends are inclusive, so an award moved today owns today on the
     * project it left <i>and</i> on the one it joined; those are two different
     * projects' usage, so nothing is double-counted.
     *
     * <p>Re-attaching to the project it is already on is a no-op. Appending a
     * second interval starting today would be harmless for ownership but would
     * misreport the history, and re-approving is routine.
     */
    public Award attach(Award award, String localProjectId, LocalDate on) {
        award.setState(Award.APPROVED);
        award.setReason("");

        Optional<Attachment> current = award.currentAttachment();

        if (current.isPresent()) {
            if (current.get().project().equals(localProjectId)) {
                save(award);

                return award;
            }

            current.get().close(on);
        }

        ObjectNode attachment = Json.object();
        attachment.put("project", localProjectId);
        attachment.put("since", on.toString());
        attachment.putNull("to");
        award.attachmentsArray().add(attachment);

        save(award);

        return award;
    }

    /**
     * Sever an award from its project, as of {@code on}.
     *
     * <p><b>The record is kept, not deleted</b> - and so is the project and its
     * usage. Removal ends the award's claim on <i>future</i> days and changes
     * nothing about the days it was already attached for, and those days still
     * have to be reportable. Deleting the record would make them unreportable,
     * or worse make the month report as empty - and an empty report is
     * vacuously complete, which would tell the allocator that nothing was ever
     * used and that this is final.
     *
     * <p>Detaching something already detached does nothing: the first date is
     * the true one, and moving it later would hand the award days it did not
     * own.
     */
    public Optional<Award> detach(String offering, String projectId, LocalDate on) {
        Optional<Award> found = award(offering, projectId);

        if (found.isEmpty()) {
            return found;
        }

        Award award = found.get();
        Optional<Attachment> current = award.currentAttachment();

        if (current.isPresent()) {
            current.get().close(on);
            save(award);
        }

        return Optional.of(award);
    }

    /** Every award we hold, across every offering. A real store would paginate. */
    public List<Award> allAwards() {
        Path awards = root.resolve("awards");

        if (!Files.isDirectory(awards)) {
            return List.of();
        }

        List<Award> found = new ArrayList<>();

        try (var offerings = Files.list(awards)) {
            List<Path> directories = offerings.sorted().toList();

            for (Path directory : directories) {
                if (!Files.isDirectory(directory)) {
                    continue;
                }

                try (var files = Files.list(directory)) {
                    for (Path file : files.sorted().toList()) {
                        String name = file.getFileName().toString();

                        if (!name.endsWith(".json")) {
                            continue;
                        }

                        found.add(new Award(
                                directory.getFileName().toString(),
                                name.substring(0, name.length() - ".json".length()),
                                read(file)));
                    }
                }
            }
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }

        return found;
    }

    /**
     * Every award made by one awarding portal <b>on one offering</b>.
     *
     * <p>Both filters matter. {@code get_awards allocator} arriving through
     * {@code cluster1} is asking what {@code allocator} has on <i>that</i>
     * resource, and an award on a different resource is no more relevant than
     * one from a different portal.
     */
    public List<Award> awardsOn(String offering, String portal) {
        List<Award> found = new ArrayList<>();

        for (Award award : allAwards()) {
            if (award.offering().equals(offering)
                    && award.projectId().endsWith("." + portal)) {
                found.add(award);
            }
        }

        return found;
    }

    /**
     * Every award that has <i>ever</i> been attached to one of our projects,
     * whether it still is or not.
     *
     * <p>The history, not the current state, because "which award owns this day"
     * is a question about the whole attachment history.
     */
    public List<Award> awardsForLocalProject(String localProjectId) {
        List<Award> found = new ArrayList<>();

        for (Award award : allAwards()) {
            if (award.projectsEverAttached().contains(localProjectId)) {
                found.add(award);
            }
        }

        return found;
    }

    /** The award currently attached to one of our projects, if any. */
    public Optional<Award> awardForLocalProjectNow(String localProjectId) {
        for (Award award : allAwards()) {
            if (localProjectId.equals(award.localProjectId().orElse(null))) {
                return Optional.of(award);
            }
        }

        return Optional.empty();
    }

    public boolean delete(String offering, String projectId) {
        try {
            return Files.deleteIfExists(awardPath(offering, projectId));
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    // ---- our own projects --------------------------------------------------

    /** One of our projects and the usage recorded against it. Never absent. */
    public LocalProject project(String localProjectId) {
        return new LocalProject(localProjectId, readOrEmpty(projectPath(localProjectId)));
    }

    public void save(LocalProject project) {
        writeAtomically(projectPath(project.localProjectId()), project.json());
    }

    // ---- paths -------------------------------------------------------------

    private Path offeringsPath() {
        return root.resolve("offerings.json");
    }

    private Path awardPath(String offering, String projectId) {
        return root.resolve("awards")
                .resolve(safe(offering, "offering"))
                .resolve(safe(projectId, "project identifier") + ".json");
    }

    private Path projectPath(String localProjectId) {
        return root.resolve("projects")
                .resolve(safe(localProjectId, "local project identifier") + ".json");
    }

    /**
     * Refuse anything that could escape the state directory.
     *
     * <p>An identifier and an offering name both arrive from the network and
     * are used here as path components. The grammar restricts them to
     * {@code [A-Za-z0-9_-.]}, so anything carrying a separator or {@code ..} is
     * not one at all - and is refused rather than allowed through.
     */
    static String safe(String component, String what) {
        if (component == null
                || component.isEmpty()
                || component.contains("/")
                || component.contains("\\")
                || component.contains("..")) {
            throw new IllegalArgumentException("unsafe " + what + ": '" + component + "'");
        }

        return component;
    }

    // ---- files -------------------------------------------------------------

    private static JsonNode read(Path path) {
        try {
            return Json.parse(Files.readString(path, StandardCharsets.UTF_8));
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }

    private static JsonNode readOrEmpty(Path path) {
        return Files.exists(path) ? read(path) : Json.object();
    }

    /**
     * Write through a temporary file and rename.
     *
     * <p>So a crash mid-write cannot leave half a record behind - the rename is
     * atomic, the write is not.
     */
    private static void writeAtomically(Path path, JsonNode data) {
        try {
            Files.createDirectories(path.getParent());

            Path temporary = Files.createTempFile(path.getParent(), null, ".tmp");

            try {
                Files.writeString(temporary, Json.write(data), StandardCharsets.UTF_8);
                Files.move(temporary, path,
                        StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
            } catch (IOException | RuntimeException e) {
                Files.deleteIfExists(temporary);

                throw e;
            }
        } catch (IOException e) {
            throw new UncheckedIOException(e);
        }
    }
}
