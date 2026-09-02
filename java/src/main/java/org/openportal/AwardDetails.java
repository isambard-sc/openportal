// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import java.time.Instant;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;

/**
 * Everything an awarding portal says about an award.
 *
 * <p>This is the argument to {@code create_award}, {@code update_award} and
 * {@code remove_award} - the JSON tail of the instruction - and the type a site
 * portal spends most of its time reading. Every field is optional, because a
 * portal fills in what it knows and because the same type is used for updates,
 * where an absent field once meant "unchanged".
 *
 * <p>Three things about reading one are worth knowing before writing a handler:
 *
 * <ul>
 *   <li><b>{@link #allocation} is the unit contract.</b> It says how much, and
 *       in whose unit, and that unit is the one every usage report about this
 *       award must come back in. An award with no allocation is not an award -
 *       refuse it. See {@link Allocation}.
 *   <li><b>{@link #template} decides what gets provisioned.</b> A site must
 *       refuse a template it does not offer, with a
 *       {@link ManagedProjectRejectedError} - not fall back to a default.
 *   <li><b>{@link #membershipControl} is permission, and absent means
 *       {@link MembershipControl#OPEN}.</b> Ask
 *       {@link #canChangeMembership()}, not the field.
 * </ul>
 *
 * <p>Mutable, with fluent setters, because a portal building one fills it in
 * field by field. {@link #toJson} writes the field set the Rust type does -
 * including the explicit {@code null}s it writes for the always-present fields,
 * because release 0.92.0 fails to read a payload that omits them.
 *
 * <p>Its wire type name is {@code "ProjectDetails"}, not
 * {@code "AwardDetails"}: the type was called that first, and the name is what
 * a {@code result_type} is matched against.
 */
public final class AwardDetails implements OpenPortalType {

    private String name;
    private ProjectTemplate template;
    private String key;
    private String description;
    private Map<String, String> members;
    private LocalDate startDate;
    private LocalDate endDate;
    private Allocation allocation;
    private final Map<String, String> breakdown = new TreeMap<>();
    private Link award;
    private Link call;
    private Link projectLink;
    private Link renewal;
    private final List<Note> notes = new ArrayList<>();
    private Instant earliestApprove;
    private MembershipControl membershipControl;
    private List<DomainPattern> allowedDomains;

    /** An empty award, to fill in with the setters. */
    public AwardDetails() {}

    public static AwardDetails fromJson(String json) {
        return fromJson(Json.parse(json));
    }

    public static AwardDetails fromJson(JsonNode node) {
        AwardDetails details = new AwardDetails();

        if (node == null || node.isNull()) {
            return details;
        }

        if (node.hasNonNull("name")) {
            details.name = node.get("name").asText();
        }

        if (node.hasNonNull("template")) {
            details.template = ProjectTemplate.parse(node.get("template").asText());
        }

        if (node.hasNonNull("key")) {
            details.key = node.get("key").asText();
        }

        if (node.hasNonNull("description")) {
            details.description = node.get("description").asText();
        }

        if (node.hasNonNull("members")) {
            Map<String, String> members = new TreeMap<>();
            node.get("members").fields().forEachRemaining(
                    entry -> members.put(entry.getKey(), entry.getValue().asText()));
            details.members = members;
        }

        if (node.hasNonNull("start_date")) {
            details.startDate = Dates.parse(node.get("start_date").asText());
        }

        if (node.hasNonNull("end_date")) {
            details.endDate = Dates.parse(node.get("end_date").asText());
        }

        if (node.hasNonNull("allocation")) {
            details.allocation = Allocation.parse(node.get("allocation").asText());
        }

        if (node.hasNonNull("breakdown")) {
            node.get("breakdown").fields().forEachRemaining(
                    entry -> details.breakdown.put(entry.getKey(), entry.getValue().asText()));
        }

        details.award = link(node, "award");
        details.call = link(node, "call");
        details.projectLink = link(node, "project_link");
        details.renewal = link(node, "renewal");

        if (node.hasNonNull("notes")) {
            node.get("notes").forEach(entry -> details.notes.add(Note.fromJson(entry)));
        }

        if (node.hasNonNull("earliest_approve")) {
            details.earliestApprove = Times.fromJson(node.get("earliest_approve"));
        }

        if (node.hasNonNull("membership_control")) {
            details.membershipControl =
                    MembershipControl.parse(node.get("membership_control").asText());
        }

        if (node.hasNonNull("allowed_domains")) {
            List<DomainPattern> domains = new ArrayList<>();
            node.get("allowed_domains").forEach(
                    entry -> domains.add(DomainPattern.parse(entry.asText())));
            details.allowedDomains = domains;
        }

        return details;
    }

    // ---- name, template, key, description ----------------------------------

    public Optional<String> name() {
        return Optional.ofNullable(name);
    }

    public AwardDetails setName(String value) {
        name = value;
        return this;
    }

    public AwardDetails clearName() {
        name = null;
        return this;
    }

    /** The template the award asks for. Refuse one you do not offer. */
    public Optional<ProjectTemplate> template() {
        return Optional.ofNullable(template);
    }

    public AwardDetails setTemplate(ProjectTemplate value) {
        template = value;
        return this;
    }

    public AwardDetails setTemplate(String value) {
        template = value == null ? null : ProjectTemplate.parse(value);
        return this;
    }

    public AwardDetails clearTemplate() {
        template = null;
        return this;
    }

    /**
     * A shared secret that shows the award may use its template.
     *
     * <p>A template name is easy to guess; the key that goes with it is not.
     * A site offering a restricted template should check this, not the name.
     */
    public Optional<String> key() {
        return Optional.ofNullable(key);
    }

    public AwardDetails setKey(String value) {
        key = value;
        return this;
    }

    public AwardDetails clearKey() {
        key = null;
        return this;
    }

    public Optional<String> description() {
        return Optional.ofNullable(description);
    }

    public AwardDetails setDescription(String value) {
        description = value;
        return this;
    }

    public AwardDetails clearDescription() {
        description = null;
        return this;
    }

    // ---- members -----------------------------------------------------------

    /**
     * Email address to role, or empty if the award says nothing about
     * membership.
     *
     * <p>Empty is not the same as an empty map: an award that carries
     * {@code "members": null} is silent about membership, and one that carries
     * {@code {}} says there are none.
     */
    public Optional<Map<String, String>> members() {
        return Optional.ofNullable(members).map(Collections::unmodifiableMap);
    }

    public AwardDetails setMembers(Map<String, String> value) {
        members = value == null ? null : new TreeMap<>(value);
        return this;
    }

    public AwardDetails addMember(String username, String role) {
        if (members == null) {
            members = new TreeMap<>();
        }

        members.put(username, role);
        return this;
    }

    public AwardDetails addMembers(Map<String, String> more) {
        more.forEach(this::addMember);
        return this;
    }

    public AwardDetails removeMember(String username) {
        if (members != null) {
            members.remove(username);
        }

        return this;
    }

    public AwardDetails clearMembers() {
        members = null;
        return this;
    }

    // ---- dates -------------------------------------------------------------

    public Optional<LocalDate> startDate() {
        return Optional.ofNullable(startDate);
    }

    public AwardDetails setStartDate(LocalDate value) {
        startDate = value;
        return this;
    }

    public AwardDetails clearStartDate() {
        startDate = null;
        return this;
    }

    public Optional<LocalDate> endDate() {
        return Optional.ofNullable(endDate);
    }

    public AwardDetails setEndDate(LocalDate value) {
        endDate = value;
        return this;
    }

    public AwardDetails clearEndDate() {
        endDate = null;
        return this;
    }

    /**
     * The earliest the receiving portal may approve this award.
     *
     * <p>A window for the awarder to make corrections between creating an award
     * and it being provisioned. A site that approves before it has ignored an
     * instruction, not merely been quick.
     */
    public Optional<Instant> earliestApprove() {
        return Optional.ofNullable(earliestApprove);
    }

    public AwardDetails setEarliestApprove(Instant value) {
        earliestApprove = value;
        return this;
    }

    public AwardDetails clearEarliestApprove() {
        earliestApprove = null;
        return this;
    }

    // ---- allocation and breakdown ------------------------------------------

    /** How much, and in whose unit. See {@link Allocation}. */
    public Optional<Allocation> allocation() {
        return Optional.ofNullable(allocation);
    }

    public AwardDetails setAllocation(Allocation value) {
        allocation = value;
        return this;
    }

    public AwardDetails setAllocation(String value) {
        allocation = value == null ? null : Allocation.parse(value);
        return this;
    }

    public AwardDetails clearAllocation() {
        allocation = null;
        return this;
    }

    /**
     * A free-form split of the allocation into named components.
     *
     * <p>Both keys and values are strings agreed out of band between the two
     * portals - OpenPortal does not interpret either, so nothing here can be
     * relied on unless the two sides have agreed it.
     */
    public Map<String, String> breakdown() {
        return Collections.unmodifiableMap(breakdown);
    }

    public AwardDetails setBreakdown(Map<String, String> value) {
        breakdown.clear();

        if (value != null) {
            breakdown.putAll(value);
        }

        return this;
    }

    public AwardDetails setBreakdownEntry(String key, String value) {
        breakdown.put(key, value);
        return this;
    }

    public AwardDetails removeBreakdownEntry(String key) {
        breakdown.remove(key);
        return this;
    }

    public AwardDetails clearBreakdown() {
        breakdown.clear();
        return this;
    }

    // ---- links -------------------------------------------------------------

    /** The award record on the funding body's system. */
    public Optional<Link> award() {
        return Optional.ofNullable(award);
    }

    public AwardDetails setAward(Link value) {
        award = value;
        return this;
    }

    public AwardDetails clearAward() {
        award = null;
        return this;
    }

    /** The funding call the award was made from. */
    public Optional<Link> call() {
        return Optional.ofNullable(call);
    }

    public AwardDetails setCall(Link value) {
        call = value;
        return this;
    }

    public AwardDetails clearCall() {
        call = null;
        return this;
    }

    /** The project's page on the awarding portal. */
    public Optional<Link> projectLink() {
        return Optional.ofNullable(projectLink);
    }

    public AwardDetails setProjectLink(Link value) {
        projectLink = value;
        return this;
    }

    public AwardDetails clearProjectLink() {
        projectLink = null;
        return this;
    }

    /** Where more time can be requested. */
    public Optional<Link> renewal() {
        return Optional.ofNullable(renewal);
    }

    public AwardDetails setRenewal(Link value) {
        renewal = value;
        return this;
    }

    public AwardDetails clearRenewal() {
        renewal = null;
        return this;
    }

    // ---- notes -------------------------------------------------------------

    /** The award's notes, oldest first. Append-only. */
    public List<Note> notes() {
        return Collections.unmodifiableList(notes);
    }

    public AwardDetails addNote(Note note) {
        notes.add(note);
        return this;
    }

    public AwardDetails clearNotes() {
        notes.clear();
        return this;
    }

    // ---- membership control ------------------------------------------------

    /**
     * The declared policy, defaulting to {@link MembershipControl#OPEN}.
     *
     * <p>Absent means open, so this never answers empty. Use
     * {@link #membershipControlIfSet} to tell a declared {@code open} from an
     * absent field.
     */
    public MembershipControl membershipControl() {
        return membershipControl == null ? MembershipControl.OPEN : membershipControl;
    }

    public Optional<MembershipControl> membershipControlIfSet() {
        return Optional.ofNullable(membershipControl);
    }

    public AwardDetails setMembershipControl(MembershipControl value) {
        membershipControl = value;
        return this;
    }

    public AwardDetails clearMembershipControl() {
        membershipControl = null;
        return this;
    }

    /** Whether this portal may add or remove members. */
    public boolean canChangeMembership() {
        return membershipControl().canChangeMembership();
    }

    /** Whether this portal may change a member's role. */
    public boolean canChangeRoles() {
        return membershipControl().canChangeRoles();
    }

    // ---- allowed domains ---------------------------------------------------

    /**
     * Who may be a member, as domain or email patterns.
     *
     * <p>Three states, and the middle one is the trap: <b>absent</b> means every
     * address is allowed, an <b>empty list</b> means none is, and a populated
     * list means only what matches. Do not collapse absent to empty.
     */
    public Optional<List<DomainPattern>> allowedDomains() {
        return Optional.ofNullable(allowedDomains).map(Collections::unmodifiableList);
    }

    public AwardDetails setAllowedDomains(List<DomainPattern> value) {
        allowedDomains = value == null ? null : new ArrayList<>(value);
        return this;
    }

    public AwardDetails addAllowedDomain(DomainPattern pattern) {
        if (allowedDomains == null) {
            allowedDomains = new ArrayList<>();
        }

        allowedDomains.add(pattern);
        return this;
    }

    public AwardDetails addAllowedDomain(String pattern) {
        return addAllowedDomain(DomainPattern.parse(pattern));
    }

    public AwardDetails clearAllowedDomains() {
        allowedDomains = null;
        return this;
    }

    /** Whether a bare domain is allowed. Email patterns are ignored. */
    public boolean isDomainAllowed(String domain) {
        if (allowedDomains == null) {
            return true;
        }

        for (DomainPattern pattern : allowedDomains) {
            if (!pattern.isEmailPattern() && pattern.matches(domain)) {
                return true;
            }
        }

        return false;
    }

    /**
     * Whether a full address is allowed.
     *
     * <p>An email pattern matches the whole address; a domain pattern matches
     * the part after the {@code @}.
     */
    public boolean isEmailAllowed(String email) {
        if (allowedDomains == null) {
            return true;
        }

        int at = email == null ? -1 : email.indexOf('@');
        String domain = at >= 0 ? email.substring(at + 1) : "";

        for (DomainPattern pattern : allowedDomains) {
            if (pattern.isEmailPattern()) {
                if (pattern.matchesEmail(email)) {
                    return true;
                }
            } else if (!domain.isEmpty() && pattern.matches(domain)) {
                return true;
            }
        }

        return false;
    }

    // ---- merge -------------------------------------------------------------

    /**
     * This award updated by {@code other}, as a new object.
     *
     * <p>Most fields are <b>replaced</b> when {@code other} sets them and left
     * alone when it does not - including {@code members} and
     * {@code allowed_domains}, which are definitive sets rather than
     * accumulating ones. An allow-list that unioned could only ever widen, and
     * could never be reduced.
     *
     * <p>Two fields accumulate on purpose: {@code breakdown} (entries are
     * merged key by key) and {@code notes} (new ones appended, then the whole
     * list re-sorted by timestamp) - the second is an audit trail.
     *
     * <p>{@code template} is the one field that can make this fail: two
     * different templates cannot be merged, because a provisioned project
     * cannot change template.
     *
     * <p>Note that the wire is moving to sending the whole award every time, so
     * a receiver should expect a complete picture rather than relying on this.
     */
    public AwardDetails merge(AwardDetails other) {
        AwardDetails merged = copy();

        if (merged.template == null) {
            merged.template = other.template;
        } else if (other.template != null && !merged.template.equals(other.template)) {
            throw new IllegalArgumentException(
                    "Cannot merge project details with different project templates: '"
                            + merged.template + "' != '" + other.template + "'");
        }

        if (other.name != null) {
            merged.name = other.name;
        }

        if (other.description != null) {
            merged.description = other.description;
        }

        if (other.startDate != null) {
            merged.startDate = other.startDate;
        }

        if (other.endDate != null) {
            merged.endDate = other.endDate;
        }

        if (other.allocation != null) {
            merged.allocation = other.allocation;
        }

        merged.breakdown.putAll(other.breakdown);

        if (other.members != null) {
            merged.members = new TreeMap<>(other.members);
        }

        if (other.key != null) {
            merged.key = other.key;
        }

        if (other.award != null) {
            merged.award = other.award;
        }

        if (other.call != null) {
            merged.call = other.call;
        }

        if (other.projectLink != null) {
            merged.projectLink = other.projectLink;
        }

        if (other.renewal != null) {
            merged.renewal = other.renewal;
        }

        for (Note note : other.notes) {
            if (!merged.notes.contains(note)) {
                merged.notes.add(note);
            }
        }

        merged.notes.sort(java.util.Comparator.comparing(Note::timestamp));

        if (other.earliestApprove != null) {
            merged.earliestApprove = other.earliestApprove;
        }

        if (other.membershipControl != null) {
            merged.membershipControl = other.membershipControl;
        }

        if (other.allowedDomains != null) {
            merged.allowedDomains = new ArrayList<>(other.allowedDomains);
        }

        return merged;
    }

    /** An independent copy - the collections included. */
    public AwardDetails copy() {
        AwardDetails copy = new AwardDetails();

        copy.name = name;
        copy.template = template;
        copy.key = key;
        copy.description = description;
        copy.members = members == null ? null : new TreeMap<>(members);
        copy.startDate = startDate;
        copy.endDate = endDate;
        copy.allocation = allocation;
        copy.breakdown.putAll(breakdown);
        copy.award = award;
        copy.call = call;
        copy.projectLink = projectLink;
        copy.renewal = renewal;
        copy.notes.addAll(notes);
        copy.earliestApprove = earliestApprove;
        copy.membershipControl = membershipControl;
        copy.allowedDomains = allowedDomains == null ? null : new ArrayList<>(allowedDomains);

        return copy;
    }

    // ---- wire form ---------------------------------------------------------

    /** {@code "ProjectDetails"} - not the name of this class. See the class docs. */
    @Override
    public String typeName() {
        return "ProjectDetails";
    }

    @Override
    public JsonNode toJson() {
        ObjectNode node = Json.object();

        // The first block is written even when null. Release 0.92.0 has no
        // `serde(default)` on these, so omitting one makes a peer of that
        // version fail outright rather than read a default.
        putOrNull(node, "name", name);
        putOrNull(node, "template", template == null ? null : template.name());
        putOrNull(node, "key", key);
        putOrNull(node, "description", description);

        if (members == null) {
            node.putNull("members");
        } else {
            ObjectNode map = node.putObject("members");
            members.forEach(map::put);
        }

        putOrNull(node, "start_date", startDate == null ? null : startDate.toString());
        putOrNull(node, "end_date", endDate == null ? null : endDate.toString());
        putOrNull(node, "allocation", allocation == null ? null : allocation.toString());

        if (!breakdown.isEmpty()) {
            ObjectNode map = node.putObject("breakdown");
            breakdown.forEach(map::put);
        }

        putLink(node, "award", award);
        putLink(node, "call", call);
        putLink(node, "project_link", projectLink);
        putLink(node, "renewal", renewal);

        ArrayNode noteArray = node.putArray("notes");
        notes.forEach(note -> noteArray.add(note.toJson()));

        if (earliestApprove != null) {
            node.put("earliest_approve", Times.toJson(earliestApprove));
        }

        if (membershipControl != null) {
            node.put("membership_control", membershipControl.wire());
        }

        if (allowedDomains == null) {
            node.putNull("allowed_domains");
        } else {
            ArrayNode domains = node.putArray("allowed_domains");
            allowedDomains.forEach(pattern -> domains.add(pattern.pattern()));
        }

        return node;
    }

    /** The JSON, which is also what the Rust side's {@code Display} gives. */
    @Override
    public String toString() {
        return Json.write(toJson());
    }

    @Override
    public boolean equals(Object other) {
        if (this == other) {
            return true;
        }

        if (!(other instanceof AwardDetails)) {
            return false;
        }

        // Compared by wire form, which is the only definition of equality the
        // two sides share.
        return toJson().equals(((AwardDetails) other).toJson());
    }

    @Override
    public int hashCode() {
        return toJson().hashCode();
    }

    private static void putOrNull(ObjectNode node, String field, String value) {
        if (value == null) {
            node.putNull(field);
        } else {
            node.put(field, value);
        }
    }

    private static void putLink(ObjectNode node, String field, Link link) {
        if (link != null) {
            node.set(field, link.toJson());
        }
    }

    private static Link link(JsonNode node, String field) {
        return node.hasNonNull(field) ? Link.fromJson(node.get(field)) : null;
    }
}
