// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.file.Path;
import java.time.LocalDate;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;
import org.openportal.Allocation;
import org.openportal.AwardDetails;
import org.openportal.DateRange;
import org.openportal.Job;
import org.openportal.Json;
import org.openportal.ProjectIdentifier;
import org.openportal.ProjectMapping;
import org.openportal.ProjectStorageReport;
import org.openportal.ProjectTemplate;
import org.openportal.ProjectUsageReport;
import org.openportal.UsageReport;

/**
 * The contract, driven without a bridge.
 *
 * <p>Every handler goes through {@link SitePortal#answer}, which returns the
 * answered job rather than posting it - so the whole contract is testable
 * against a job built by hand. That is the reason {@code answer} and
 * {@code handle} are separate methods.
 *
 * <p>The cast. {@code allocator} is the awarding portal, {@code site} is us, and
 * {@code cluster1} and {@code cluster2} are two resources we offer - virtual
 * agents on this portal that {@code allocator} addresses directly. Two awards,
 * on two resources, each mapped to a project of ours:
 *
 * <pre>
 *     allocator.site.cluster1   myaward1.allocator  ->  myproject1.site
 *     allocator.site.cluster2   myaward2.allocator  ->  myproject2.site
 * </pre>
 *
 * <p>Most checks use the first pair. {@code cluster2} is here because with one
 * resource the most important property of an offering is invisible: an award
 * lives on one resource, and the other knows nothing about it.
 */
class SitePortalTest {

    private static final String OFFERING = "cluster1";
    private static final String OTHER = "cluster2";
    private static final String AWARD = "myaward1.allocator";
    private static final String LOCAL = "myproject1.site";
    private static final String LEAD = "alice@example.com";
    private static final String MEMBER = "bob@example.com";

    /** What {@code cluster1} and {@code allocator} agreed: one NHR is 4 GPUHR. */
    private static final Map<String, Double> AGREED = Map.of("GPUHR", 4.0);

    /** {@code cluster2} agreed a different factor, which is the point of it. */
    private static final Map<String, Double> AGREED_OTHER = Map.of("GPUHR", 2.0);

    private Store store;
    private SitePortal portal;

    @BeforeEach
    void setUp(@TempDir Path state) {
        store = new Store(state);
        portal = new SitePortal(store);
    }

    // ---- building a job by hand --------------------------------------------

    /**
     * A job as the bridge would deliver one.
     *
     * <p>{@code forwarded_for} is what names the offering, and it is set by our
     * own portal agent rather than by the caller - which is why it is the field
     * worth trusting. Its first element is the portal that asked; its last is
     * the offering they came in through.
     */
    private Job job(String command, String offering) {
        var raw = Json.object();
        raw.put("id", "00000000-0000-0000-0000-000000000001");
        raw.put("created", 0);
        raw.put("changed", 0);
        raw.put("expires", 0);
        raw.put("version", 1);
        raw.put("command", "site.site_bridge." + offering + " " + command);

        // `Running`, because that is the state a job handed to a portal is in -
        // and the state a completion is allowed from.
        raw.put("state", "Running");
        raw.putNull("result");
        raw.putNull("result_type");
        raw.put("forwarded_for", "allocator.site." + offering);

        return Job.of(raw);
    }

    private Job job(String command) {
        return job(command, OFFERING);
    }

    /** An award as the awarding portal would send one. */
    private static AwardDetails details(String template, String allocation) {
        return new AwardDetails()
                .setName("Example award")
                .setTemplate(new ProjectTemplate(template))
                .setAllocation(allocation == null ? null : Allocation.parse(allocation))
                .addMember(LEAD, "lead");
    }

    private static AwardDetails details() {
        return details("standard", "5000 GPUHR");
    }

    private Job created(String award, AwardDetails details, String offering) {
        return portal.answer(
                job("create_project " + award + " " + Json.write(details.toJson()), offering));
    }

    private Job created(AwardDetails details) {
        return created(AWARD, details, OFFERING);
    }

    private Award approve(String local, LocalDate on) {
        Award award = store.award(OFFERING, AWARD).orElseThrow();

        return store.attach(award, local, on);
    }

    private void setUsage(String local, Map<LocalDate, Map<String, Double>> usage) {
        store.save(store.project(local).setUsage(usage));
    }

    private static Map<LocalDate, Map<String, Double>> usage(
            LocalDate date, String email, double hours) {
        Map<String, Double> perUser = new LinkedHashMap<>();
        perUser.put(email, hours);

        Map<LocalDate, Map<String, Double>> usage = new LinkedHashMap<>();
        usage.put(date, perUser);

        return usage;
    }

    // ---- offerings ---------------------------------------------------------

    @Test
    void a_fresh_portal_offers_nothing_and_that_is_a_valid_state() {
        // Not a misconfiguration - a site that advertises nothing simply cannot
        // be asked for anything yet.
        assertEquals(List.of(), portal.offeringNames());
    }

    @Test
    void offerings_are_state_rather_than_configuration() {
        portal.addOffering(OFFERING, List.of("standard", "large"), AGREED);
        portal.addOffering(OTHER, List.of("standard"), AGREED_OTHER);

        assertEquals(List.of(OFFERING, OTHER), portal.offeringNames());
        assertEquals(java.util.Set.of("standard", "large"), portal.templatesFor(OFFERING));

        // An upsert, because the operator API is retried like everything else:
        // adding a resource you already have changes it rather than failing.
        portal.addOffering(OFFERING, List.of("standard"), null);
        assertEquals(java.util.Set.of("standard"), portal.templatesFor(OFFERING));

        // ...and omitting the conversions keeps what was agreed, so templates
        // can be changed on their own.
        assertEquals(4.0, portal.conversionsFor(OFFERING).get("GPUHR"));

        assertEquals(2, portal.offerings().size());
    }

    @Test
    void an_offering_name_that_could_not_be_part_of_a_destination_is_refused() {
        // It becomes one element of a Destination, so it is checked where an
        // operator types it rather than failing later inside a destination
        // nobody is looking at.
        for (String bad : List.of("-cluster", "my cluster", "cluster.1", "", "a".repeat(65))) {
            assertThrows(IllegalArgumentException.class,
                    () -> portal.addOffering(bad, List.of("standard"), null),
                    "should have refused '" + bad + "'");
        }
    }

    @Test
    void a_resource_with_no_templates_is_refused() {
        // What a resource can be asked for is the site's decision about that
        // resource. Defaulting it would publish a guess under the site's name
        // that an awarding portal could not tell from a policy.
        assertThrows(IllegalArgumentException.class,
                () -> portal.addOffering("cluster9", List.of(), AGREED));

        assertFalse(portal.offeringNames().contains("cluster9"));
    }

    @Test
    void withdrawing_an_offering_keeps_the_awards_on_it() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        portal.removeOffering(OFFERING);

        // Withdrawn, not deleted. Those awards still own the days they were
        // attached for, and deleting them would make a later report empty - and
        // an empty report is vacuously complete.
        assertEquals(List.of(), portal.offeringNames());
        assertEquals(1, store.allAwards().size());
    }

    // ---- the award decisions -----------------------------------------------

    @Test
    void a_new_award_is_answered_pending_and_kept() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = created(details());

        // The error *is* the answer: there is no mapping to return yet, only a
        // reason. `award_pending` means "ask again", and the allocator will.
        assertTrue(answered.isError());
        assertEquals("award_pending", answered.errorKind());
        assertEquals(1, store.allAwards().size());
        assertEquals(Award.PENDING, store.award(OFFERING, AWARD).orElseThrow().state());
    }

    @Test
    void the_same_award_arriving_again_is_not_a_second_project() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        created(details());
        created(details().setName("renamed"));

        // The awarding portal re-sends it every cycle to re-assert its state.
        assertEquals(1, store.allAwards().size());

        // ...and the incoming details are merged over what we hold, so a
        // changed name or member list takes effect.
        assertEquals("renamed",
                store.award(OFFERING, AWARD).orElseThrow().details().name().orElseThrow());
    }

    @Test
    void a_template_we_do_not_offer_is_refused_terminally() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = created(details("enormous", "5000 GPUHR"));

        // Rejected, not pending: asking again cannot help, so the allocator
        // should stop rather than retrying forever.
        assertTrue(answered.isError());
        assertEquals("award_rejected", answered.errorKind());
        assertEquals(0, store.allAwards().size());
    }

    @Test
    void an_award_that_awards_nothing_is_not_an_award() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        // No allocation at all. There is no amount to provision against and -
        // because the allocation is what names the unit - no way to say what any
        // usage we later reported would mean.
        Job none = created(details("standard", null));
        assertEquals("award_rejected", none.errorKind());
        assertTrue(none.errorMessage().contains("no allocation"), none.errorMessage());

        // And an allocation of zero, which is the same thing spelt differently.
        Job zero = created(details("standard", "0 GPUHR"));
        assertEquals("award_rejected", zero.errorKind());
        assertTrue(zero.errorMessage().contains("awards nothing"), zero.errorMessage());

        assertEquals(0, store.allAwards().size());
    }

    @Test
    void an_award_in_a_unit_we_have_no_agreed_factor_for_is_refused() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = created(details("standard", "5000 CPUHR"));

        // The alternative is to accept it and then answer every usage report
        // with a well-formed zero. There is no safe factor to guess: 1.0 would
        // report a quarter of the usage and 0 would report none.
        assertEquals("award_rejected", answered.errorKind());
        assertTrue(answered.errorMessage().contains("no agreed conversion"),
                answered.errorMessage());

        // An award in the site's own unit needs no agreement, and is accepted.
        assertEquals("award_pending", created(details("standard", "1000 NHR")).errorKind());
    }

    @Test
    void an_approved_award_answers_with_the_mapping_that_joins_the_two_names() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());
        approve(LOCAL, LocalDate.now());

        Job answered = created(details());

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals("ProjectMapping", answered.resultType().orElseThrow());

        ProjectMapping mapping = answered.resultText(ProjectMapping::parse).orElseThrow();

        // Their name for the award, and ours for the project. Neither side can
        // guess the other's, so this is where they are joined.
        assertEquals(AWARD, mapping.project().toString());
        assertEquals(LOCAL, mapping.localGroup());
    }

    @Test
    void a_rejected_award_stays_rejected_every_cycle() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        Award award = store.award(OFFERING, AWARD).orElseThrow();
        award.setState(Award.REJECTED);
        award.setReason("not this year");
        store.save(award);

        Job answered = created(details());

        assertEquals("award_rejected", answered.errorKind());
        assertTrue(answered.errorMessage().contains("not this year"));
    }

    @Test
    void an_update_for_an_award_we_have_never_seen_goes_through_the_approval_path() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = portal.answer(job(
                "update_project " + AWARD + " " + Json.write(details().toJson())));

        // A missed message or a rebuilt database gets us here. Treating it as a
        // create routes it through approval rather than silently provisioning
        // something nobody approved.
        assertEquals("award_pending", answered.errorKind());
        assertEquals(Award.PENDING, store.award(OFFERING, AWARD).orElseThrow().state());
    }

    @Test
    void both_spellings_of_every_award_instruction_are_answered() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        // The wire is moving from `*_project` to `*_award`, so a table keyed on
        // only one of the two starts answering "unsupported" on the day it
        // changes.
        for (String command : List.of("create_project", "create_award",
                "update_project", "update_award")) {
            Job answered = portal.answer(job(
                    command + " " + AWARD + " " + Json.write(details().toJson())));

            assertFalse("unsupported".equals(answered.errorKind()),
                    command + " should be answered");
        }

        for (String command : List.of("remove_project", "remove_award")) {
            assertFalse(portal.answer(job(command + " " + AWARD)).isError(), command);
        }
    }

    // ---- reading awards back -----------------------------------------------

    @Test
    void get_award_returns_the_details_with_the_members_in_them() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details().addMember(MEMBER, "member"));
        approve(LOCAL, LocalDate.now());

        Job answered = portal.answer(job("get_award " + AWARD));

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals("ProjectDetails", answered.resultType().orElseThrow());

        AwardDetails read = answered.result(AwardDetails::fromJson).orElseThrow();

        // This portal does not implement `get_users`; members travel with the
        // award instead, and this is the field callers read.
        assertEquals(java.util.Set.of(LEAD, MEMBER), read.members().orElseThrow().keySet());
    }

    @Test
    void get_projects_answers_mappings_and_a_detached_award_maps_to_None() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());
        approve(LOCAL, LocalDate.now());

        Job answered = portal.answer(job("get_projects allocator"));

        assertFalse(answered.isError(), answered.errorMessage());

        // A list, and named as one: `Vec<T>`, not `T`. The awarding portal
        // deserialises the JSON against the name it was given.
        assertEquals("Vec<ProjectMapping>", answered.resultType().orElseThrow());
        assertEquals("[\"" + AWARD + ":" + LOCAL + "\"]",
                answered.resultText().orElseThrow());

        portal.answer(job("remove_project " + AWARD));

        // Still approved - it was approved once and that did happen - but with
        // no project attached to name.
        assertEquals("[\"" + AWARD + ":None\"]",
                portal.answer(job("get_projects allocator")).resultText().orElseThrow());
    }

    @Test
    void an_empty_listing_is_still_named_as_a_list() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        // The element type comes from the handler rather than the first value,
        // because an empty list has no first value - and an empty
        // `get_projects` is still a list of mappings.
        Job answered = portal.answer(job("get_projects allocator"));

        assertEquals("Vec<ProjectMapping>", answered.resultType().orElseThrow());
        assertEquals("[]", answered.resultText().orElseThrow());

        assertEquals("Vec<ProjectDetails>",
                portal.answer(job("get_awards allocator")).resultType().orElseThrow());
    }

    // ---- the offering is part of an award's identity -----------------------

    @Test
    void an_award_on_one_resource_is_invisible_from_the_other() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        portal.addOffering(OTHER, List.of("standard"), AGREED_OTHER);
        created(details());
        approve(LOCAL, LocalDate.now());

        // Same identifier, different resource: a question about a different
        // thing, and the honest answer is nothing rather than an error.
        Job answered = portal.answer(job("get_usage_report " + AWARD + " today", OTHER));

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals(0,
                answered.result(ProjectUsageReport::fromJson).orElseThrow()
                        .totalUsage().seconds());

        // ...whereas asking for the award itself finds nothing, and that is an
        // OpenPortalError rather than a rejection: we are not refusing this
        // award, we simply do not have it here.
        Job missing = portal.answer(job("get_award " + AWARD, OTHER));

        assertTrue(missing.isError());
        assertEquals("unknown", missing.errorKind());
        assertInstanceOf(org.openportal.OpenPortalError.class, missing.error().orElseThrow());
    }

    @Test
    void a_request_through_an_offering_we_do_not_advertise_is_refused() {
        // A backstop rather than the access control: the portal agent only
        // forwards requests for offerings we registered.
        Job answered = portal.answer(job("get_projects allocator", "cluster9"));

        assertEquals("award_rejected", answered.errorKind());
    }

    // ---- usage reports -----------------------------------------------------

    @Test
    void a_usage_report_is_converted_into_the_awards_unit_and_their_namespace() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        LocalDate today = LocalDate.now();
        approve(LOCAL, today);
        setUsage(LOCAL, usage(today, LEAD, 12.5));

        Job answered = portal.answer(job("get_usage_report " + AWARD + " today"));

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals("ProjectUsageReport", answered.resultType().orElseThrow());

        ProjectUsageReport report =
                answered.result(ProjectUsageReport::fromJson).orElseThrow();

        // 12.5 of our node hours, at the agreed four to one, is 50 of their GPU
        // hours. Reporting 12.5 would not be reporting slightly differently - it
        // would be reporting a different quantity under the same name.
        assertEquals(50.0, report.totalUsage().hours(), 0.001);

        // And the report is about *their* award, with our project nowhere in it.
        assertEquals(AWARD, report.project().toString());
        assertEquals(List.of("alice." + AWARD),
                report.users().stream().map(Object::toString).toList());

        // The same usage on a resource that agreed two to one is 25 instead,
        // which is the whole point of the factor being per-resource.
        portal.addOffering(OTHER, List.of("standard"), AGREED_OTHER);
        assertEquals(2.0, portal.conversionsFor(OTHER).get("GPUHR"));
    }

    @Test
    void what_an_award_is_worth_here_is_the_same_factor_the_other_way() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        // 5000 of their GPU hours, at four to one, is 1250 node hours to spend
        // here - which is the number this site would enforce a quota against.
        assertEquals(1250.0,
                portal.toSiteUnits(OFFERING, Allocation.parse("5000 GPUHR")).orElseThrow(),
                0.001);

        // No agreed factor, no answer - rather than a guess.
        assertTrue(portal.toSiteUnits(OFFERING, Allocation.parse("5000 CPUHR")).isEmpty());
    }

    @Test
    void completeness_is_a_decision_rather_than_a_calendar_comparison() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        LocalDate day = LocalDate.of(2026, 8, 3);
        approve(LOCAL, day);
        setUsage(LOCAL, usage(day, LEAD, 4.0));

        DateRange august = DateRange.parse("2026-08-01:2026-08-31");

        // Until the site declares the month final the report is incomplete, and
        // the allocator keeps asking. Nothing in the code can know the figures
        // will not change.
        ProjectUsageReport report = portal.buildUsageReport(OFFERING, AWARD, august);
        assertFalse(report.isComplete());
        assertEquals(16.0, report.totalUsage().hours(), 0.001);

        store.save(store.project(LOCAL).setFinal("2026-08", true));

        assertTrue(portal.buildUsageReport(OFFERING, AWARD, august).isComplete());
    }

    @Test
    void a_month_with_no_figures_yet_gets_an_incomplete_placeholder() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());
        approve(LOCAL, LocalDate.of(2026, 8, 1));

        // `isComplete` is "every day I contain is complete", which is vacuously
        // **true** for a report with no days at all. So a month we have simply
        // not ingested yet would otherwise answer "nothing was used, and that is
        // final" - and the allocator would believe it and stop asking.
        ProjectUsageReport report = portal.buildUsageReport(
                OFFERING, AWARD, DateRange.parse("2026-09-01:2026-09-30"));

        assertEquals(0, report.totalUsage().seconds());
        assertFalse(report.isComplete(), "an unreported month must not claim to be settled");
        assertFalse(report.dates().isEmpty(), "there has to be a day to carry the flag");
    }

    @Test
    void a_month_wholly_outside_the_awards_attachments_is_correctly_empty() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        // Attached from September, so August was never this award's to own.
        // "Nothing, and that is final" is the truth here rather than an accident.
        approve(LOCAL, LocalDate.of(2026, 9, 1));

        ProjectUsageReport report = portal.buildUsageReport(
                OFFERING, AWARD, DateRange.parse("2026-08-01:2026-08-31"));

        assertEquals(0, report.totalUsage().seconds());
        assertTrue(report.dates().isEmpty());
    }

    @Test
    void get_usage_reports_rolls_up_to_the_portal() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        LocalDate today = LocalDate.now();
        approve(LOCAL, today);
        setUsage(LOCAL, usage(today, LEAD, 1.0));

        Job answered = portal.answer(job("get_usage_reports allocator today"));

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals("UsageReport", answered.resultType().orElseThrow());

        UsageReport report = answered.result(UsageReport::fromJson).orElseThrow();

        assertEquals("allocator", report.portal().toString());
        assertEquals(4.0, report.totalUsage().hours(), 0.001);
    }

    @Test
    void a_portal_with_no_awards_rolls_up_to_an_empty_report_rather_than_failing() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = portal.answer(job("get_usage_reports allocator today"));

        assertFalse(answered.isError(), answered.errorMessage());
        assertTrue(answered.result(UsageReport::fromJson).orElseThrow().isEmpty());
    }

    @Test
    void usage_for_an_unapproved_award_is_empty_rather_than_an_error() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        Job answered = portal.answer(job("get_usage_report " + AWARD + " today"));

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals(0,
                answered.result(ProjectUsageReport::fromJson).orElseThrow()
                        .totalUsage().seconds());
    }

    // ---- removal, and the days an award keeps ------------------------------

    @Test
    void remove_award_severs_the_link_and_keeps_the_days_already_owned() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        LocalDate yesterday = LocalDate.now().minusDays(1);
        approve(LOCAL, yesterday);
        setUsage(LOCAL, usage(yesterday, LEAD, 2.0));

        Job answered = portal.answer(job("remove_project " + AWARD));

        // There is no longer a project attached to name.
        assertEquals(AWARD + ":None", answered.resultText(ProjectMapping::parse)
                .orElseThrow().toString());

        Award award = store.award(OFFERING, AWARD).orElseThrow();
        assertFalse(award.isAttached());

        // The record and the figures are kept, because the days the award
        // already owns still have to be reportable - the allocator has not
        // necessarily collected the final ones yet, and it cannot ask a question
        // we have destroyed the answer to.
        assertEquals(8.0, portal.buildUsageReport(OFFERING, AWARD,
                        DateRange.of(yesterday, LocalDate.now()))
                .totalUsage().hours(), 0.001);
    }

    @Test
    void removing_an_award_we_do_not_hold_is_not_an_error() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        // The caller wants it gone, and it is gone. Being idempotent means a
        // retried removal does not produce a spurious failure.
        Job answered = portal.answer(job("remove_project " + AWARD));

        assertFalse(answered.isError(), answered.errorMessage());
        assertEquals(AWARD + ":None", answered.resultText(ProjectMapping::parse)
                .orElseThrow().toString());
    }

    @Test
    void a_removed_award_re_asserted_goes_back_to_the_pending_queue() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());
        approve(LOCAL, LocalDate.now().minusDays(2));
        portal.answer(job("remove_project " + AWARD));

        Job answered = created(details());

        // The allocator still holds it, so it is asking us to attach it to a
        // project - a fresh decision for an operator, not something to
        // resurrect on the old project's behalf. Pending, not rejected: there
        // is nothing wrong with the award.
        assertEquals("award_pending", answered.errorKind());
        assertEquals(Award.PENDING, store.award(OFFERING, AWARD).orElseThrow().state());

        // ...and the attachment history is left exactly as it is: the days it
        // owned before are still its days.
        assertEquals(1, store.award(OFFERING, AWARD).orElseThrow().attachments().size());
    }

    @Test
    void a_day_belongs_to_the_award_last_attached_during_it() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        LocalDate today = LocalDate.now();
        String second = "myaward2.allocator";

        created(details());
        approve(LOCAL, today.minusDays(2));

        // A handover: the first award is detached today and the second attached
        // today, on the same project.
        created(second, details(), OFFERING);
        store.detach(OFFERING, AWARD, today);
        store.attach(store.award(OFFERING, second).orElseThrow(), LOCAL, today);

        Map<LocalDate, Map<String, Double>> figures = new LinkedHashMap<>();
        figures.putAll(usage(today.minusDays(1), LEAD, 1.0));
        figures.putAll(usage(today, LEAD, 3.0));
        setUsage(LOCAL, figures);

        DateRange window = DateRange.of(today.minusDays(2), today);

        // The whole of today goes to the later attachment, not just the part
        // after the handover: usage is accounted per day, so a day is
        // indivisible.
        assertEquals(4.0,
                portal.buildUsageReport(OFFERING, AWARD, window).totalUsage().hours(), 0.001);
        assertEquals(12.0,
                portal.buildUsageReport(OFFERING, second, window).totalUsage().hours(), 0.001);
    }

    @Test
    void a_day_the_project_was_attached_to_nothing_is_billed_to_nobody() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());

        LocalDate today = LocalDate.now();
        approve(LOCAL, today.minusDays(5));
        store.detach(OFFERING, AWARD, today.minusDays(3));

        // Usage after the detachment is real and stays in our own accounting;
        // there is simply no award for it to appear under.
        setUsage(LOCAL, usage(today, LEAD, 10.0));

        assertEquals(0, portal.buildUsageReport(OFFERING, AWARD,
                DateRange.of(today.minusDays(5), today)).totalUsage().seconds());
    }

    // ---- storage, and what is not implemented ------------------------------

    @Test
    void a_portal_with_no_storage_answers_an_empty_report_rather_than_failing() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = portal.answer(job("get_storage_report " + AWARD + " today"));

        // Empty says "nothing here"; an error says "something is broken", and
        // only the first is true.
        assertFalse(answered.isError(), answered.errorMessage());
        assertTrue(answered.result(ProjectStorageReport::fromJson).orElseThrow().isEmpty());
    }

    @Test
    void an_instruction_this_portal_does_not_implement_says_so() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        Job answered = portal.answer(job("get_users allocator"));

        // A legitimate answer: a portal implements as much of the contract as
        // it has answers for. `get_users` is deliberately absent - members
        // travel in the award instead.
        assertTrue(answered.isError());
        assertEquals("unsupported", answered.errorKind());
    }

    @Test
    void every_job_gets_an_answer_even_when_a_handler_has_a_bug() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);

        // A malformed identifier gets past the split and into a handler, where
        // parsing it throws something that is not an OpenPortalError. The job is
        // still answered, so the caller learns something went wrong rather than
        // waiting two minutes for it to expire.
        Job answered = portal.answer(job("get_award not-an-identifier"));

        assertTrue(answered.isError());
        assertFalse(answered.errorMessage().isBlank());
        assertTrue(answered.isFinished());
    }

    @Test
    void the_local_project_identifier_is_ours_and_the_award_identifier_is_theirs() {
        portal.addOffering(OFFERING, List.of("standard"), AGREED);
        created(details());
        approve(LOCAL, LocalDate.now());

        Award award = store.award(OFFERING, AWARD).orElseThrow();

        // Two names for one object, once the mapping has been returned.
        assertEquals(LOCAL, award.localProjectId().orElseThrow());
        assertEquals(AWARD, award.projectId());
        assertEquals("site", ProjectIdentifier.parse(LOCAL).portal());
        assertEquals("allocator", ProjectIdentifier.parse(AWARD).portal());
    }
}
