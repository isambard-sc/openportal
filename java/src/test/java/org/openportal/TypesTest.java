// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.time.Instant;
import java.time.LocalDate;
import java.util.List;
import java.util.Map;
import org.junit.jupiter.api.Test;

/**
 * The type wrappers against the Rust implementation they have to agree with.
 *
 * <p>Every expected value here was produced by running the published
 * {@code openportal} Python module - which is the Rust types through pyo3 - and
 * pasted in. That is the point: a wrapper that merely round-trips through
 * itself proves nothing, because both halves can be wrong together. What is
 * pinned is the <b>strings and JSON the other side actually writes</b>.
 */
class TypesTest {

    // ---- identifiers -------------------------------------------------------

    @Test
    void identifiers_parse_and_print_back() {
        UserIdentifier user = UserIdentifier.parse("alice.myproject.allocator");

        assertEquals("alice", user.username());
        assertEquals("myproject", user.project());
        assertEquals("allocator", user.portal());
        assertEquals("alice.myproject.allocator", user.toString());
        assertEquals("myproject.allocator", user.projectIdentifier().toString());
        assertEquals("allocator", user.portalIdentifier().toString());
    }

    @Test
    void an_identifier_with_the_wrong_number_of_components_is_refused() {
        // The count is the whole grammar - two for a project, three for a user -
        // so a user identifier passed where a project one belongs has to fail
        // rather than silently taking the first two parts.
        assertThrows(IllegalArgumentException.class,
                () -> ProjectIdentifier.parse("alice.myproject.allocator"));
        assertThrows(IllegalArgumentException.class,
                () -> UserIdentifier.parse("myproject.allocator"));

        // A trailing dot is an empty component, not a shorter identifier.
        assertThrows(IllegalArgumentException.class,
                () -> ProjectIdentifier.parse("myproject."));
    }

    @Test
    void the_identifier_charset_is_an_allow_list() {
        // Each of these is admitted by a deny-list and matters: a space shifts
        // every later argument of an instruction string, a comma is a list
        // separator to sacctmgr, and a ? starts a query in a Slurm REST URL.
        for (String bad : List.of("my project", "my,project", "my=project", "my?project",
                "my#project", "my%project", "-myproject")) {
            assertThrows(IllegalArgumentException.class,
                    () -> ProjectIdentifier.parse(bad + ".allocator"),
                    "should have refused '" + bad + "'");
        }
    }

    @Test
    void a_mapping_target_may_contain_dots_but_not_at_the_ends() {
        // A local account derived from user.project is legitimately named that.
        UserMapping mapping = UserMapping.parse("alice.proj.allocator:alice.proj:projgroup");

        assertEquals("alice.proj", mapping.localUser());
        assertEquals("projgroup", mapping.localGroup());
        assertEquals("alice.proj.allocator:alice.proj:projgroup", mapping.toString());

        // As a path component these resolve to the current or parent directory.
        assertThrows(IllegalArgumentException.class,
                () -> UserMapping.parse("alice.proj.allocator:.alice:grp"));
        assertThrows(IllegalArgumentException.class,
                () -> UserMapping.parse("alice.proj.allocator:a..b:grp"));
    }

    @Test
    void a_mapping_from_a_portal_carries_an_email_where_a_unix_name_would_be() {
        // A portal has no Unix accounts to name, so it reports the member's
        // email in the same position. Both forms travel in one field.
        UserMapping mapping = UserMapping.parse("alice.proj.allocator:alice@example.com:projgroup");

        assertTrue(mapping.localUserIsEmail());
        assertEquals("alice@example.com", mapping.localUser());

        // And the value must not reach anything that spawns a process.
        assertThrows(IllegalStateException.class, mapping::unixLocalUser);

        UserMapping unix = UserMapping.parse("alice.proj.allocator:alice:projgroup");
        assertFalse(unix.localUserIsEmail());
        assertEquals("alice", unix.unixLocalUser());
    }

    // ---- allocation --------------------------------------------------------

    @Test
    void allocation_prints_as_the_rust_side_does() {
        // The size goes on the wire inside a string, so "5000.0 GPUHR" would
        // round-trip and still not compare equal to what was sent.
        assertEquals("5000 GPUHR", Allocation.parse("5000 GPUHR").toString());
        assertEquals("12.5 NHR", Allocation.parse("12.5 NHR").toString());
        assertEquals("0.001 NHR", Allocation.parse("0.001 NHR").toString());
        assertEquals("1000000 CPUHR", Allocation.parse("1000000 CPUHR").toString());
        assertEquals("1000 NHR", Allocation.parse("1e3 NHR").toString());
        assertEquals("No allocation", Allocation.empty().toString());
    }

    @Test
    void allocation_units_are_canonicalised_only_for_the_six_known_names() {
        assertEquals("3 GPUHR", Allocation.parse("3 GPU hours").toString());
        assertEquals("NHR", Allocation.canonicalize("node hour"));
        assertEquals("BHR", Allocation.canonicalize("billing hours"));

        // Anything else is lower-cased rather than left alone, which is the
        // surprise: "CHR" is not a known unit, so it becomes "chr" - and "chr"
        // is then what the other side compares against.
        assertEquals("chr", Allocation.canonicalize("CHR"));
        assertEquals("2.5 credits", Allocation.parse("2.5 credits").toString());
    }

    @Test
    void allocation_refuses_the_values_that_would_saturate_downstream() {
        // Double.parseDouble accepts these, and a `< 0` test is false for NaN,
        // so both used to parse cleanly and then saturate to the maximum.
        assertThrows(IllegalArgumentException.class, () -> Allocation.parse("NaN NHR"));
        assertThrows(IllegalArgumentException.class, () -> Allocation.parse("Infinity NHR"));
        assertThrows(IllegalArgumentException.class, () -> Allocation.parse("-5 NHR"));

        // No separator: not a size of 5000.
        assertThrows(IllegalArgumentException.class, () -> Allocation.parse("5000GPUHR"));
    }

    @Test
    void converting_against_a_node_that_cannot_express_the_unit_raises() {
        Node gpuless = Node.of(2, 64, 0, 512000, 100);
        Allocation gpuHours = Allocation.parse("4000 GPUHR");

        // The failure that matters: answering zero here would provision an award
        // with nothing, and nobody would see an error.
        assertThrows(IllegalStateException.class, () -> gpuHours.toNodeHours(gpuless));

        Node node = Node.of(2, 64, 4, 512000, 100);
        assertEquals(1000.0, gpuHours.toNodeHours(node).hours(), 0.001);
        assertEquals(4000.0, gpuHours.toGpuHours(node).hours(), 0.001);
    }

    @Test
    void node_reports_its_shape_as_the_rust_side_prints_it() {
        Node node = Node.of(2, 64, 4, 512000, 100);

        assertEquals(128, node.cores());
        assertEquals(500.0, node.memoryGb(), 0.0);
        assertEquals("Node(cpus: 2, cores_per_cpu: 64, gpus: 4, memory: 500 GB, billing: 100)",
                node.toString());
    }

    // ---- usage -------------------------------------------------------------

    @Test
    void usage_prints_in_the_largest_unit_it_reaches() {
        assertEquals("0 seconds", new Usage(0).toString());
        assertEquals("1 second", new Usage(1).toString());
        assertEquals("59 seconds", new Usage(59).toString());
        assertEquals("1.000 minute", new Usage(60).toString());
        assertEquals("1.500 minutes", new Usage(90).toString());
        assertEquals("1.000 hour", new Usage(3600).toString());
        assertEquals("1.500 hours", new Usage(5400).toString());
        assertEquals("1.000 day", new Usage(86400).toString());
        assertEquals("1.042 days", new Usage(90000).toString());
        assertEquals("1.000 week", new Usage(604800).toString());
        assertEquals("1.142 months", new Usage(3000000).toString());
        assertEquals("1.268 years", new Usage(40000000).toString());
    }

    @Test
    void usage_in_hours_is_the_stable_rendering() {
        assertEquals("0.000 hours", new Usage(0).inHours());
        assertEquals("0.016 hours", new Usage(59).inHours());
        assertEquals("25.000 hours", new Usage(90000).inHours());
        assertEquals("11111.111 hours", new Usage(40000000).inHours());
    }

    @Test
    void usage_is_an_object_on_the_wire_not_a_number() {
        assertEquals("{\"seconds\":7200}", Json.write(Usage.fromHours(2).toJson()));

        // Read back from both forms - a hand-written report is the likeliest
        // source of a bare number.
        assertEquals(7200, Usage.fromJson(Json.parse("{\"seconds\":7200}")).seconds());
        assertEquals(7200, Usage.fromJson(Json.parse("7200")).seconds());
    }

    @Test
    void usage_arithmetic_saturates_rather_than_wrapping_or_going_negative() {
        // On the Rust side an overflow under `overflow-checks` with
        // `panic = "abort"` is a process kill, and these values come from a peer.
        assertEquals(Long.MAX_VALUE,
                new Usage(Long.MAX_VALUE).plus(new Usage(1000)).seconds());

        // There is no negative usage.
        assertEquals(0, new Usage(100).minus(new Usage(500)).seconds());

        // And dividing by zero answers zero rather than throwing.
        assertEquals(0, new Usage(3600).dividedBy(0).seconds());
    }

    // ---- storage -----------------------------------------------------------

    @Test
    void storage_size_prints_with_binary_units_and_inclusive_boundaries() {
        // Exactly 1024 bytes is still bytes; 1025 is the first kilobyte. The
        // same one-off applies at every boundary, which is why 1 TB prints in GB.
        assertEquals("0 B", new StorageSize(0).toString());
        assertEquals("1024 B", new StorageSize(1024).toString());
        assertEquals("1.00 KB", new StorageSize(1025).toString());
        assertEquals("1024.00 KB", new StorageSize(1048576).toString());
        assertEquals("1.00 MB", new StorageSize(1048577).toString());
        assertEquals("1024.00 MB", new StorageSize(1073741824).toString());
        assertEquals("1.00 GB", new StorageSize(1073741825L).toString());
        assertEquals("1024.00 GB", new StorageSize(1099511627776L).toString());
        assertEquals("1.00 TB", new StorageSize(1099511627777L).toString());
        assertEquals("1024.00 TB", new StorageSize(1125899906842624L).toString());
        assertEquals("2.00 PB", new StorageSize(2251799813685248L).toString());
    }

    @Test
    void storage_size_parses_with_or_without_a_space_but_needs_a_unit() {
        assertEquals(2199023255552L, StorageSize.parse("2TB").bytes());
        assertEquals(2199023255552L, StorageSize.parse("2 TB").bytes());
        assertEquals(536870912000L, StorageSize.parse("500gigabytes").bytes());
        assertEquals(1610612736L, StorageSize.parse("1.5GB").bytes());

        // A bare number is refused, matching the Rust side - it has no arm for
        // an empty unit.
        assertThrows(IllegalArgumentException.class, () -> StorageSize.parse("100"));
    }

    @Test
    void a_quota_without_a_measurement_declines_to_answer_rather_than_saying_zero() {
        Quota limitOnly = Quota.parse("100GB");

        assertFalse(limitOnly.isOverQuota());
        assertTrue(limitOnly.percentageUsed().isEmpty());
        assertEquals("100.00 GB", limitOnly.toString());

        Quota measured = Quota.parse("100GB used 50GB");
        assertEquals("50.00 GB / 100.00 GB | 50.0%", measured.toString());
        assertEquals(50.0, measured.percentageUsed().getAsDouble(), 0.001);
        assertFalse(measured.isOverQuota());

        assertTrue(Quota.parse("1TB used 2TB").isOverQuota());
        assertEquals("2.00 TB / 1024.00 GB | 200.0%", Quota.parse("1TB used 2TB").toString());
    }

    @Test
    void unlimited_is_a_state_not_a_large_number() {
        Quota unlimited = Quota.parse("unlimited");

        assertTrue(unlimited.isUnlimited());
        assertFalse(unlimited.isOverQuota());
        assertTrue(unlimited.limit().size().isEmpty());
        assertEquals("unlimited", unlimited.toString());

        // And it sorts above every size.
        assertTrue(QuotaLimit.unlimited()
                .compareTo(QuotaLimit.limited(StorageSize.parse("1PB"))) > 0);
    }

    @Test
    void a_quota_omits_its_usage_from_the_json_when_it_has_none() {
        assertEquals("{\"limit\":\"100.00 GB\"}", Json.write(Quota.parse("100GB").toJson()));
        assertEquals("{\"limit\":\"100.00 GB\",\"usage\":\"50.00 GB\"}",
                Json.write(Quota.parse("100GB used 50GB").toJson()));
    }

    // ---- dates -------------------------------------------------------------

    @Test
    void a_date_range_is_inclusive_as_dates_and_half_open_as_instants() {
        DateRange range = DateRange.parse("2026-09-01:2026-09-03");

        assertEquals("2026-09-01:2026-09-03", range.toString());
        assertEquals(List.of(
                LocalDate.of(2026, 9, 1), LocalDate.of(2026, 9, 2), LocalDate.of(2026, 9, 3)),
                range.days());

        // The end is the following midnight, which is what makes a day's usage
        // belong to exactly one day.
        assertEquals("2026-09-04T00:00", range.endTime().toString());
    }

    @Test
    void a_single_date_is_a_one_day_range_and_a_reversed_one_is_straightened() {
        assertEquals("2026-09-01:2026-09-01", DateRange.parse("2026-09-01").toString());
        assertEquals("2026-09-01:2026-09-03",
                DateRange.parse("2026-09-03:2026-09-01").toString());
    }

    @Test
    void a_date_range_is_bounded_in_both_year_and_span() {
        // %Y accepts an unbounded digit count, and the span is what decides how
        // many days a report iterates over.
        assertThrows(IllegalArgumentException.class, () -> DateRange.parse("1900-01-01"));
        assertThrows(IllegalArgumentException.class, () -> DateRange.parse("3000-01-01"));
        assertThrows(IllegalArgumentException.class,
                () -> DateRange.parse("2000-01-01:2100-01-01"));
    }

    @Test
    void the_whole_periods_a_range_touches_extend_beyond_it() {
        DateRange range = DateRange.of(LocalDate.of(2026, 1, 30), LocalDate.of(2026, 2, 2));

        assertEquals(List.of("2026-01-01:2026-01-31", "2026-02-01:2026-02-28"),
                range.months().stream().map(DateRange::toString).toList());

        // 2026-01-30 is a Friday, so its week starts on the Monday before.
        assertEquals("2026-01-26:2026-02-01", range.weeks().get(0).toString());
    }

    // ---- awards ------------------------------------------------------------

    /** Written by the Python module, field for field. */
    private static final String AWARD_JSON = """
            {"name":"proj-x","template":"standard","key":"k1","description":"desc",\
            "members":{"alice@example.com":"member"},"start_date":"2026-01-01",\
            "end_date":"2026-12-31","allocation":"5000 GPUHR",\
            "breakdown":{"gpu":"1000 GPUHR"},\
            "award":{"id":"AW1","url":"https://x/AW1"},\
            "notes":[{"timestamp":"2026-09-02T08:28:06.602549301Z","author":"bob",\
            "text":"hello"}],"earliest_approve":"2026-01-02T03:04:05Z",\
            "membership_control":"members_only","allowed_domains":["example.com"]}""";

    @Test
    void an_award_written_by_the_python_module_round_trips_unchanged() {
        AwardDetails award = AwardDetails.fromJson(AWARD_JSON);

        // Read...
        assertEquals("proj-x", award.name().orElseThrow());
        assertEquals("standard", award.template().orElseThrow().name());
        assertEquals("5000 GPUHR", award.allocation().orElseThrow().toString());
        assertEquals(Map.of("alice@example.com", "member"), award.members().orElseThrow());
        assertEquals(LocalDate.of(2026, 1, 1), award.startDate().orElseThrow());
        assertEquals("AW1", award.award().orElseThrow().id());
        assertEquals(1, award.notes().size());
        assertEquals("hello", award.notes().get(0).text());
        assertEquals(Instant.parse("2026-01-02T03:04:05Z"), award.earliestApprove().orElseThrow());
        assertEquals(MembershipControl.MEMBERS_ONLY, award.membershipControl());

        // ...and written back byte for byte, nanoseconds on the note included.
        assertEquals(Json.parse(AWARD_JSON), award.toJson());
    }

    @Test
    void an_empty_award_still_writes_the_fields_a_0_92_peer_requires() {
        // Not a smaller payload but an unreadable one: release 0.92.0 has no
        // serde(default) on these, so omitting one makes it fail outright.
        assertEquals(
                "{\"name\":null,\"template\":null,\"key\":null,\"description\":null,"
                        + "\"members\":null,\"start_date\":null,\"end_date\":null,"
                        + "\"allocation\":null,\"notes\":[],\"allowed_domains\":null}",
                Json.write(new AwardDetails().toJson()));
    }

    @Test
    void the_awards_wire_type_name_is_not_the_name_of_its_class() {
        // The type was called ProjectDetails first, and result_type is matched
        // against the name the Rust side registers.
        assertEquals("ProjectDetails", new AwardDetails().typeName());
    }

    @Test
    void an_absent_membership_control_means_open_not_refusal() {
        AwardDetails award = AwardDetails.fromJson("{}");

        assertEquals(MembershipControl.OPEN, award.membershipControl());
        assertTrue(award.membershipControlIfSet().isEmpty());
        assertTrue(award.canChangeMembership());
        assertTrue(award.canChangeRoles());

        // The two asymmetric ones, which is why nobody should compare values.
        award.setMembershipControl(MembershipControl.MEMBERS_ONLY);
        assertTrue(award.canChangeMembership());
        assertFalse(award.canChangeRoles());

        award.setMembershipControl(MembershipControl.ROLES_ONLY);
        assertFalse(award.canChangeMembership());
        assertTrue(award.canChangeRoles());
    }

    @Test
    void an_absent_allowed_domains_permits_everything_and_an_empty_one_permits_nothing() {
        assertTrue(AwardDetails.fromJson("{}").isEmailAllowed("anyone@anywhere.com"));

        AwardDetails none = new AwardDetails().setAllowedDomains(List.of());
        assertFalse(none.isEmailAllowed("anyone@anywhere.com"));

        AwardDetails some = new AwardDetails()
                .addAllowedDomain("example.com")
                .addAllowedDomain("*.ac.uk")
                .addAllowedDomain("named@elsewhere.org");

        assertTrue(some.isEmailAllowed("alice@example.com"));
        assertTrue(some.isEmailAllowed("bob@bristol.ac.uk"));
        assertTrue(some.isEmailAllowed("named@elsewhere.org"));
        assertFalse(some.isEmailAllowed("other@elsewhere.org"));

        // A wildcard matches subdomains at any depth, and not the bare domain.
        assertTrue(DomainPattern.parse("*.example.com").matches("a.b.example.com"));
        assertFalse(DomainPattern.parse("*.example.com").matches("example.com"));

        // An email pattern is not a domain pattern, in either direction.
        assertFalse(DomainPattern.parse("named@elsewhere.org").matches("elsewhere.org"));
        assertFalse(DomainPattern.parse("elsewhere.org").matchesEmail("named@elsewhere.org"));
    }

    @Test
    void a_link_refuses_a_scheme_that_would_be_dangerous_to_render() {
        // These links are documented as being for a portal UI, and a URL parser
        // accepts javascript: and file: happily.
        assertThrows(IllegalArgumentException.class,
                () -> Link.of("x", "javascript:alert(1)"));
        assertThrows(IllegalArgumentException.class, () -> Link.of("x", "file:///etc/passwd"));

        assertEquals("{\"id\":\"AW1\",\"url\":\"https://x/AW1\"}",
                Link.of("AW1", "https://x/AW1").toString());
        assertEquals("{}", Link.empty().toString());
    }

    @Test
    void merging_an_award_replaces_sets_and_accumulates_the_audit_trail() {
        AwardDetails first = new AwardDetails()
                .setTemplate("standard")
                .setName("original")
                .setMembers(Map.of("alice@example.com", "member"))
                .setBreakdownEntry("gpu", "500 GPUHR")
                .addAllowedDomain("example.com")
                .addNote(Note.at(Instant.parse("2026-01-01T00:00:00Z"), "alice", "first"));

        AwardDetails update = new AwardDetails()
                .setMembers(Map.of("bob@example.com", "member"))
                .setBreakdownEntry("cpu", "100 CPUHR")
                .setAllowedDomains(List.of())
                .addNote(Note.at(Instant.parse("2026-01-02T00:00:00Z"), "bob", "second"));

        AwardDetails merged = first.merge(update);

        // Definitive sets are replaced: an allow-list that unioned could only
        // ever widen, and could never be reduced to nothing.
        assertEquals(Map.of("bob@example.com", "member"), merged.members().orElseThrow());
        assertEquals(List.of(), merged.allowedDomains().orElseThrow());

        // The two that accumulate on purpose.
        assertEquals(Map.of("gpu", "500 GPUHR", "cpu", "100 CPUHR"), merged.breakdown());
        assertEquals(List.of("first", "second"),
                merged.notes().stream().map(Note::text).toList());

        // Unset fields are left alone.
        assertEquals("original", merged.name().orElseThrow());
    }

    @Test
    void two_different_templates_cannot_be_merged() {
        // A provisioned project cannot change template, so this is the one
        // field a merge can refuse.
        AwardDetails standard = new AwardDetails().setTemplate("standard");
        AwardDetails large = new AwardDetails().setTemplate("large");

        assertThrows(IllegalArgumentException.class, () -> standard.merge(large));
    }

    // ---- usage reports -----------------------------------------------------

    /** Written by the Python module from one day's usage plus a mapping. */
    private static final String PROJECT_REPORT_JSON = """
            {"project":"proj.site","reports":{"2026-09-01":{"reports":{"alice":{"seconds":7200},\
            "unknown":{"seconds":1800}},"components":{"cpu":{"alice":{"seconds":3600}}},\
            "user_job_counts":{},"user_wait_seconds":{},"num_jobs":0,"total_wait_seconds":0,\
            "is_complete":true}},"users":{"alice.proj.site":"alice"}}""";

    @Test
    void a_project_report_written_by_the_python_module_reads_back() {
        ProjectUsageReport report = ProjectUsageReport.fromJson(PROJECT_REPORT_JSON);

        assertEquals("proj.site", report.project().toString());
        assertEquals(List.of(LocalDate.of(2026, 9, 1)), report.dates());
        assertEquals(List.of("cpu"), report.components());
        assertTrue(report.isComplete());

        // Nine thousand seconds: two attributed hours plus half an unattributed
        // one. The unattributed share counts towards the total.
        assertEquals(9000, report.totalUsage().seconds());
        assertEquals(7200, report.usage(UserIdentifier.parse("alice.proj.site")).seconds());
        assertEquals(1800, report.unmappedUsage().seconds());
        assertEquals(List.of("unknown"), report.unmappedUsers());

        assertEquals(Json.parse(PROJECT_REPORT_JSON), report.toJson());
    }

    @Test
    void a_report_a_site_builds_carries_the_two_fields_a_0_92_peer_requires() {
        DailyProjectUsageReport day = new DailyProjectUsageReport()
                .addUsage("alice", Usage.fromHours(2))
                .addComponentUsage("cpu", "alice", Usage.fromHours(1))
                .addUnattributedUsage(Usage.fromMinutes(30))
                .setComplete();

        ProjectUsageReport report = new ProjectUsageReport(ProjectIdentifier.parse("proj.site"))
                .addMapping(UserMapping.parse("alice.proj.site:alice:grp"))
                .setReport(LocalDate.of(2026, 9, 1), day);

        // `reports` and `is_complete` on the day, and `users` on the project,
        // are written even when empty: release 0.92.0 has no serde(default) on
        // those three, so omitting one makes a peer of that version fail
        // outright rather than read a default. Every other empty map is
        // omitted - those do have defaults.
        assertEquals("{\"project\":\"proj.site\","
                        + "\"reports\":{\"2026-09-01\":{"
                        + "\"reports\":{\"alice\":{\"seconds\":7200},"
                        + "\"unknown\":{\"seconds\":1800}},"
                        + "\"is_complete\":true,"
                        + "\"components\":{\"cpu\":{\"alice\":{\"seconds\":3600}}}}},"
                        + "\"users\":{\"alice.proj.site\":\"alice\"}}",
                Json.write(report.toJson()));

        // The 0.92.0 wheel writes four more empty fields for the same input -
        // `user_job_counts`, `user_wait_seconds`, `num_jobs` and
        // `total_wait_seconds`, which later releases skip. Both forms have to
        // read back to the same report, in both directions, or a Java site and
        // a Python one disagree about a report neither of them changed.
        ProjectUsageReport fromOlder = ProjectUsageReport.fromJson(PROJECT_REPORT_JSON);

        assertEquals(report.totalUsage(), fromOlder.totalUsage());
        assertEquals(report.components(), fromOlder.components());
        assertEquals(report.userMapping(), fromOlder.userMapping());
        assertEquals(report.numJobs(), fromOlder.numJobs());
        assertEquals(report.isComplete(), fromOlder.isComplete());
    }

    @Test
    void a_portal_report_refuses_a_project_belonging_to_another_portal() {
        // A report whose keys disagree with its own portal field is one a
        // receiver that trusts the keys will mis-attribute.
        UsageReport report = new UsageReport(new PortalIdentifier("site"));

        assertThrows(IllegalArgumentException.class, () -> report.setReport(
                new ProjectUsageReport(ProjectIdentifier.parse("proj.elsewhere"))));

        assertThrows(IllegalArgumentException.class, () -> UsageReport.fromJson(
                "{\"portal\":\"site\",\"reports\":{\"proj.elsewhere\":"
                        + "{\"project\":\"proj.elsewhere\",\"reports\":{},\"users\":{}}}}"));
    }

    @Test
    void a_report_converted_into_the_awards_unit_scales_every_figure() {
        // The site accounts in node hours; the award is in GPU hours at four to
        // one. Converting is one multiplication, and the components have to
        // move with the totals or the report adds two units together.
        DailyProjectUsageReport day = new DailyProjectUsageReport()
                .addUsage("alice", Usage.fromHours(12.5))
                .addComponentUsage("gpu", "alice", Usage.fromHours(12.5));

        DailyProjectUsageReport converted = day.times(4.0);

        assertEquals(50.0, converted.totalUsage().hours(), 0.001);
        assertEquals(50.0, converted.getComponent("gpu").totalUsage().hours(), 0.001);
    }

    @Test
    void remapping_two_local_users_onto_one_sums_them_rather_than_losing_one() {
        // Consolidating two local accounts must not silently lose either one's
        // usage - and which one it lost would depend on map order.
        ProjectUsageReport report = new ProjectUsageReport(ProjectIdentifier.parse("proj.site"))
                .addMapping(UserMapping.parse("alice.proj.site:alice:grp"))
                .addMapping(UserMapping.parse("bob.proj.site:bob:grp"))
                .setReport(LocalDate.of(2026, 9, 1), new DailyProjectUsageReport()
                        .addUsage("alice", Usage.fromHours(1))
                        .addUsage("bob", Usage.fromHours(2)));

        ProjectUsageReport remapped = report.remapUsers(Map.of(
                UserIdentifier.parse("alice.proj.site"), "shared",
                UserIdentifier.parse("bob.proj.site"), "shared"));

        assertEquals(3.0, remapped.totalUsage().hours(), 0.001);
        assertEquals(3.0, remapped.localUsage("shared").hours(), 0.001);
    }

    @Test
    void remapping_a_project_moves_its_users_with_it() {
        // A UserIdentifier names a user *within* a project, so the mapping keys
        // are stale the moment the project is renamed.
        ProjectUsageReport report = new ProjectUsageReport(ProjectIdentifier.parse("local.site"))
                .addMapping(UserMapping.parse("alice.local.site:alice:grp"));

        ProjectUsageReport remapped =
                report.remapProject(ProjectIdentifier.parse("theiraward.allocator"));

        assertEquals("theiraward.allocator", remapped.project().toString());
        assertEquals(List.of("alice.theiraward.allocator"),
                remapped.users().stream().map(UserIdentifier::toString).toList());
    }

    @Test
    void a_days_scalar_totals_have_to_agree_with_the_maps_they_shadow() {
        DailyProjectUsageReport day = new DailyProjectUsageReport()
                .addJobs("alice", 3)
                .addWaitSeconds("alice", 90);

        assertEquals(3, day.numJobs());
        assertEquals(3, day.numJobsForUser("alice"));
        assertEquals(30, day.averageWaitSecondsForUser("alice"));
        assertTrue(day.isConsistent());

        // Data from an older instance has no maps to check against, and is
        // consistent by definition rather than by inspection.
        assertTrue(DailyProjectUsageReport
                .fromJson("{\"reports\":{},\"num_jobs\":17,\"is_complete\":true}")
                .isConsistent());
    }

    @Test
    void a_day_keeps_the_fields_this_client_does_not_model() {
        // The wire type carries about thirty fields. Rebuilding one from named
        // fields would drop whatever had not been modelled - silently.
        String withRequeues = "{\"reports\":{\"alice\":{\"seconds\":1800}},"
                + "\"requeue_reports\":{\"alice\":{\"seconds\":7200}},"
                + "\"requeue_states\":{\"NODE_FAIL\":2},\"num_requeue_events\":2,"
                + "\"is_complete\":true}";

        DailyProjectUsageReport day = DailyProjectUsageReport.fromJson(withRequeues);

        assertEquals(1800, day.totalUsage().seconds());
        assertEquals(7200, day.totalRequeueUsage().seconds());
        assertEquals(9000, day.totalUsageIncludingRequeues().seconds());
        assertEquals(Json.parse(withRequeues), day.toJson());

        // And they are summed and scaled along with everything else.
        assertEquals(14400, day.plus(day).totalRequeueUsage().seconds());
        assertEquals(3600, day.times(0.5).totalRequeueUsage().seconds());
    }

    // ---- storage reports ---------------------------------------------------

    @Test
    void a_storage_report_is_a_snapshot_with_a_history_behind_it() {
        ProjectIdentifier project = ProjectIdentifier.parse("proj.site");
        Volume home = Volume.parse("home");

        ProjectStorageReport older = new ProjectStorageReport(project)
                .setGeneratedAt(Instant.parse("2026-09-01T12:00:00Z"))
                .setProjectQuota(home, Quota.parse("1TB used 100GB"));

        ProjectStorageReport newer = new ProjectStorageReport(project)
                .setGeneratedAt(Instant.parse("2026-09-02T12:00:00Z"))
                .setProjectQuota(home, Quota.parse("1TB used 200GB"));

        ProjectStorageReport combined = older.plus(newer);

        // The newest snapshot is the top-level one, and it is a level rather
        // than an amount - so nothing is summed.
        assertEquals(LocalDate.of(2026, 9, 2), combined.date());
        assertEquals("200.00 GB",
                combined.projectQuotas().get(home).usageIfSet().orElseThrow().toString());
        assertEquals(2, combined.dailyReports().size());
        assertEquals("100.00 GB", combined.getReport(LocalDate.of(2026, 9, 1))
                .projectQuotas().get(home).usageIfSet().orElseThrow().toString());
    }

    @Test
    void a_storage_report_round_trips_its_json() {
        String json = "{\"project\":\"proj.site\",\"generated_at\":\"2026-09-02T08:48:45.737140172Z\","
                + "\"project_quotas\":{},\"user_quotas\":{},\"users\":{}}";

        assertEquals(Json.parse(json), ProjectStorageReport.fromJson(json).toJson());
    }

    // ---- notifications -----------------------------------------------------

    @Test
    void a_notification_spells_its_event_differently_in_json_and_as_a_string() {
        // Externally tagged on the wire, snake_case as a string. Dispatch on
        // eventType, which is always the snake_case name.
        String json = "{\"id\":\"085a58e0-9e32-4679-bab9-1473d13ad1c2\","
                + "\"destination\":\"portal.clusters.shared\","
                + "\"event\":{\"UserAdded\":\"chris.p.portal\"},"
                + "\"domain\":\"greatwestern\",\"domain_version\":\"0.92.0\"}";

        Notification notification = Notification.fromJson(json);

        assertEquals("user_added", notification.eventType());
        assertEquals("chris.p.portal", notification.eventArgument());
        assertEquals("user_added chris.p.portal", notification.event());
        assertEquals("portal.clusters.shared", notification.destination());
        assertEquals("chris.p.portal", notification.user().toString());

        // Read and written back unchanged, the domain fields this client does
        // not model included.
        assertEquals(Json.parse(json), notification.toJson());
    }

    @Test
    void parsing_a_notification_refuses_an_event_name_it_does_not_know() {
        Notification parsed =
                Notification.parse("portal.clusters.shared award_accepted proj.allocator");

        assertEquals("award_accepted", parsed.eventType());
        assertEquals("proj.allocator", parsed.projectOrAward().toString());
        assertEquals("{\"AwardAccepted\":\"proj.allocator\"}",
                Json.write(parsed.toJson().get("event")));

        // Stamped with the domain that owns the vocabulary, as an agent's own
        // notification is.
        assertEquals("greatwestern", parsed.domain().orElseThrow());

        // A typo fails where it is written rather than being delivered as an
        // event nobody handles.
        assertThrows(IllegalArgumentException.class,
                () -> Notification.parse("portal user_addded chris.p.portal"));

        // And the infrastructure-only event has no string form at all.
        assertThrows(IllegalArgumentException.class,
                () -> Notification.parse("portal forward something"));
    }

    // ---- instructions ------------------------------------------------------

    @Test
    void an_instruction_hands_back_its_arguments_typed() {
        Instruction instruction = Instruction.parse(
                "create_award myaward1.allocator " + AWARD_JSON);

        assertEquals("create_award", instruction.command());
        assertEquals("myaward1.allocator", instruction.projectIdentifier(0).toString());
        assertEquals("5000 GPUHR",
                instruction.awardDetails().allocation().orElseThrow().toString());

        // remove_award carries no JSON, and that is an empty award rather than
        // a failure.
        assertTrue(Instruction.parse("remove_award myaward1.allocator")
                .awardDetails().allocation().isEmpty());

        // A missing argument names what was wanted.
        OpenPortalError missing = assertThrows(OpenPortalError.class,
                () -> Instruction.parse("get_usage_report").dateRange(0));
        assertTrue(missing.getMessage().contains("a date range"), missing.getMessage());
    }
}
