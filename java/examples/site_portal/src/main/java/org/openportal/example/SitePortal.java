// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import java.time.LocalDate;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import java.util.TreeSet;
import java.util.function.DoubleFunction;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.openportal.Allocation;
import org.openportal.AwardDetails;
import org.openportal.DailyProjectUsageReport;
import org.openportal.DateRange;
import org.openportal.Destination;
import org.openportal.Instruction;
import org.openportal.Job;
import org.openportal.ManagedProjectPendingError;
import org.openportal.ManagedProjectRejectedError;
import org.openportal.Note;
import org.openportal.OpenPortalError;
import org.openportal.OpenPortalType;
import org.openportal.OpenPortalUnsupportedCommandError;
import org.openportal.PortalIdentifier;
import org.openportal.ProjectIdentifier;
import org.openportal.ProjectMapping;
import org.openportal.ProjectStorageReport;
import org.openportal.ProjectUsageReport;
import org.openportal.StorageReport;
import org.openportal.Usage;
import org.openportal.UsageReport;
import org.openportal.UserIdentifier;
import org.openportal.UserMapping;

/**
 * The contract: what this portal answers, and how.
 *
 * <p>This is the file to read. Everything OpenPortal asks of a site portal is
 * here, one method per instruction, in the order
 * {@code docs/specifications/site-portal-api.md} §4 lists them.
 *
 * <p>The shape to take away is {@link #answer} at the bottom: <b>every job gets
 * an answer.</b> A handler either returns a value or throws; either way a result
 * is posted. A job left unanswered is indistinguishable from an outage until it
 * expires two minutes later, and it is the one failure mode worth designing out
 * structurally rather than remembering to avoid.
 */
public final class SitePortal {

    private static final Logger LOG = Logger.getLogger(SitePortal.class.getName());

    /**
     * The unit this site accounts in - what the figures pushed to the operator
     * API are in, and what its own records hold.
     *
     * <p>Node hours here, and worth being honest about how notional that is: a
     * real site with heterogeneous clusters may measure a scheduler billing unit
     * underneath and present a <i>hypothetical</i> node-hour equivalent to its
     * users. The contract does not care. It needs one unit, named, that every
     * figure this portal reports is expressed in.
     */
    public static final String SITE_UNIT = "NHR";

    /**
     * What an offering may be called.
     *
     * <p>It becomes one element of a {@link Destination}, so it is what the
     * grammar allows for an agent name and nothing more - checked here, at the
     * point an operator types it, rather than failing later inside a destination
     * nobody is looking at.
     */
    static final java.util.regex.Pattern OFFERING_NAME =
            java.util.regex.Pattern.compile("^[A-Za-z0-9_][A-Za-z0-9_-]{0,63}$");

    private final Store store;

    public SitePortal(Store store) {
        this.store = store;
    }

    public Store store() {
        return store;
    }

    // ----------------------------------------------------------------------
    // What this portal offers
    // ----------------------------------------------------------------------

    // An offering is a **virtual agent** on this portal: a name the awarding
    // portal addresses directly, standing for one resource we run. On the wire
    // it is written `<offering>.<us>.<them>` - the resource, offered by us, to
    // them - and `App` registers the full paths with the bridge.
    //
    // **The offering is part of an award's identity, not a permission check.**
    // An award created through `cluster1` is an award *on `cluster1`*. The same
    // awarding portal can hold a different award of the same name on
    // `cluster2`, and asking one resource about a project that lives on the
    // other gets nothing back. Which is why running this with two resources
    // teaches more than running it with one.
    //
    // The set lives in the store rather than in a constant, because it is
    // state: a site procures a cluster, retires one, or opens one to a second
    // awarding portal, and none of those are code changes. A fresh portal
    // therefore offers **nothing** until an operator adds a resource - which is
    // not a misconfiguration, just a site that cannot be asked for anything yet.

    /** Every resource we advertise, in name order. */
    public List<Offering> offerings() {
        return new ArrayList<>(store.offerings().values());
    }

    /** Just the names, which is what most callers want. */
    public List<String> offeringNames() {
        return new ArrayList<>(store.offerings().keySet());
    }

    /** The templates one resource accepts - empty if we do not offer it. */
    public java.util.Set<String> templatesFor(String offering) {
        return store.offering(offering)
                .map(found -> (java.util.Set<String>) new TreeSet<>(found.templates()))
                .orElseGet(TreeSet::new);
    }

    /**
     * Start advertising a resource, or change its templates or conversions.
     *
     * <p>The templates are <b>required</b>: what a resource can be asked for is
     * the site's decision about that resource, and defaulting it would publish a
     * guess under the site's name that an awarding portal could not tell from a
     * policy.
     */
    public Offering addOffering(
            String name, List<String> templates, Map<String, Double> conversions) {
        if (name == null || !OFFERING_NAME.matcher(name).matches()) {
            throw new IllegalArgumentException("'" + name + "' is not a usable offering name -"
                    + " use 1-64 characters of A-Za-z0-9_- not starting with '-'");
        }

        if (templates == null || templates.isEmpty()) {
            throw new IllegalArgumentException("name the templates this resource accepts -"
                    + " an award naming one you do not offer is refused, and there is no default");
        }

        for (String template : templates) {
            // Parsed rather than pattern-matched, so the rule is the one the
            // wire type applies rather than a second copy of it.
            org.openportal.ProjectTemplate.parse(template);
        }

        if (conversions != null) {
            for (Map.Entry<String, Double> agreed : conversions.entrySet()) {
                if (!(agreed.getValue() > 0.0) || !Double.isFinite(agreed.getValue())) {
                    throw new IllegalArgumentException("the conversion for '" + agreed.getKey()
                            + "' has to be a positive number, not " + agreed.getValue());
                }
            }
        }

        Map<String, Double> canonical = null;

        if (conversions != null) {
            canonical = new TreeMap<>();

            for (Map.Entry<String, Double> agreed : conversions.entrySet()) {
                canonical.put(canonicalUnit(agreed.getKey()), agreed.getValue());
            }
        }

        return store.addOffering(name, templates, canonical, LocalDate.now());
    }

    /** Stop advertising a resource. The awards on it are kept - see the store. */
    public Optional<Offering> removeOffering(String name) {
        return store.removeOffering(name);
    }

    // ----------------------------------------------------------------------
    // Units: what a figure in a usage report means
    // ----------------------------------------------------------------------

    // An award is for a *quantity*, and the two portals do not have to count in
    // the same thing. The awarding portal allocates in its unit; this site
    // accounts in its own; the two agree a factor between them, once, out of
    // band.
    //
    //     N allocator units awarded  ->  M site units to spend here
    //     X site units used          ->  Y allocator units reported back
    //
    // That is the whole of it, and it is deliberately the whole of it. How a
    // site turns its own unit into real hardware - cores, GPUs, memory, a
    // scheduler's billing weight - is the site's own business logic and no part
    // of this contract. Nor are the units necessarily hours: they are numbers
    // with an agreed name, and one day they may be money or cloud credits
    // without any of the logic above changing.
    //
    // Converting on the way out is the same kind of act as remapping the
    // identifiers, and it happens in the same place for the same reason: a
    // report is built from what we recorded and then translated into what the
    // other portal understands.

    /**
     * A unit name in the spelling {@link Allocation} uses.
     *
     * <p>{@code Allocation} canonicalises the ones it knows - "gpu hours",
     * "GPUhr" and {@code GPUHR} are one unit - and passes anything else through
     * lower-cased. Both sides of every comparison below go through this, so an
     * agreed unit of {@code CREDITS} matches an award allocated in
     * {@code credits}.
     */
    static String canonicalUnit(String unit) {
        return Allocation.canonicalize(unit == null ? "" : unit);
    }

    /**
     * The agreed factors for one resource: allocator unit → how many of them one
     * {@link #SITE_UNIT} is worth.
     *
     * <p>{@code {"GPUHR": 4.0}} reads "one of our node hours is four of their
     * GPU hours", so an award of 5000 GPUHR is 1250 node hours to spend here,
     * and 12.5 node hours used is 50 GPU hours to report back.
     *
     * <p>Our own unit is always in the table at 1.0 - if an awarding portal
     * allocates in the unit we already count in, there is nothing to agree.
     */
    public Map<String, Double> conversionsFor(String offering) {
        Map<String, Double> agreed = new TreeMap<>();
        agreed.put(canonicalUnit(SITE_UNIT), 1.0);

        store.offering(offering).ifPresent(found ->
                found.conversions().forEach((unit, factor) ->
                        agreed.put(canonicalUnit(unit), factor)));

        return agreed;
    }

    /**
     * A function turning a figure in <i>our</i> unit into the award's unit, or
     * empty when there is no agreed factor.
     *
     * <p><b>This is what decides what the numbers in a usage report mean.</b> A
     * {@link Usage} is a bare number; nothing in it says whether 50 is 50 node
     * hours or 50 GPU hours. The unit is the one the awarding portal allocated
     * in, so a site that reports its own figures unconverted is not reporting
     * slightly differently - it is reporting a different quantity under the same
     * name, and nothing on the wire will catch it.
     *
     * <p>Empty must be treated as "we cannot hold this award" rather than as
     * zero or as one-for-one. Guessing 1.0 would silently report a quarter of
     * the usage; guessing 0 would report none. <b>There is no safe default for a
     * number whose meaning was never agreed.</b>
     */
    public Optional<DoubleFunction<Usage>> converterFor(String offering, Allocation allocation) {
        if (allocation == null || allocation.isEmpty()) {
            // No allocation means no unit, and no award either - `createAward`
            // refuses one, so this is a record predating that check rather than
            // something to convert. There is nothing honest to return.
            return Optional.empty();
        }

        Double factor = conversionsFor(offering)
                .get(canonicalUnit(allocation.units()));

        if (factor == null || factor == 0.0) {
            return Optional.empty();
        }

        double agreed = factor;

        return Optional.of(hours -> Usage.fromHours(hours * agreed));
    }

    /**
     * The other direction: what an award is worth here, in {@link #SITE_UNIT}.
     *
     * <p>The same agreed factor, divided rather than multiplied. This is the
     * number a site actually enforces against - a quota, a budget, a limit in
     * its own scheduler - and it is worth showing because it makes the round
     * trip visible: 5000 of their units in, 1250 of ours to spend, and every
     * report back multiplied by four again.
     */
    public Optional<Double> toSiteUnits(String offering, Allocation allocation) {
        if (allocation == null || allocation.isEmpty() || allocation.size() == null) {
            return Optional.empty();
        }

        Double factor = conversionsFor(offering).get(canonicalUnit(allocation.units()));

        if (factor == null || factor == 0.0) {
            return Optional.empty();
        }

        return Optional.of(allocation.size() / factor);
    }

    // ----------------------------------------------------------------------
    // Small helpers
    // ----------------------------------------------------------------------

    /**
     * Our own project identifier for an award, which only an award that is
     * <i>currently attached</i> to a project of ours has.
     */
    private ProjectIdentifier localProject(Award award) {
        return ProjectIdentifier.parse(award.localProjectId().orElseThrow(() ->
                new OpenPortalError(award.projectId() + " is not attached to a project here")));
    }

    /**
     * The {@link ProjectMapping} most award instructions return:
     * {@code <their project id>:<our project id>}.
     *
     * <p><b>This is the whole point of the exchange.</b> The awarding portal
     * knows the award as {@code myaward1.allocator}; we know the project we
     * attached it to as {@code myproject1.site}. Neither side can guess the
     * other's name, so the mapping is where they are joined - and once it has
     * been returned, both sides know that their award and our project are the
     * same object.
     *
     * <p>It matters beyond bookkeeping. Our accounting produces usage figures
     * for {@code myproject1.site} and has never heard of
     * {@code myaward1.allocator}; the mapping is what lets
     * {@code get_usage_report} answer a question asked in their namespace with
     * figures recorded in ours.
     *
     * <p>Only ever built for an <i>approved</i> award. One still awaiting
     * approval has no local project, so there is no honest identifier to put
     * here - which is precisely why the answer in that case is an error.
     */
    private ProjectMapping mapping(Award award) {
        return ProjectMapping.parse(award.projectId() + ":" + localProject(award));
    }

    /**
     * Which resource this request is about.
     *
     * <p>Every request arrives addressed to one of our virtual agents, and that
     * name is the last element of the path. {@code forwarded_for} carries the
     * original {@code allocator.site.cluster1} when the request came from
     * another portal; the job's own destination is {@code site.<bridge>.cluster1}
     * and ends the same way, so it is the fallback for a locally-originated
     * request.
     *
     * <p>This is not decoration. It scopes everything below: an award belongs to
     * the offering it was created on, and a question asked of a different
     * offering is a question about a different thing.
     */
    static String offeringOf(Job job) {
        return job.forwardedFor().orElseGet(job::destination).last();
    }

    /**
     * An award we hold <b>on the offering this request came through</b>, or a
     * clear failure.
     *
     * <p>{@link OpenPortalError}, not {@link ManagedProjectRejectedError}: we
     * are not refusing this award, we simply do not have it here. The
     * distinction matters because a rejection is terminal to the caller.
     */
    private Award requireAward(Job job, String projectId) {
        String offering = offeringOf(job);

        return store.award(offering, projectId).orElseThrow(() ->
                new OpenPortalError("no award " + projectId + " on " + offering));
    }

    /**
     * Our local approval state, as the contract's answer.
     *
     * <p>The most important method in this example, because getting it wrong is
     * costly in both directions:
     *
     * <ul>
     *   <li>{@link ManagedProjectPendingError} means <i>not yet, ask again</i>.
     *       The awarding portal logs it quietly and retries next cycle. An award
     *       parked here for a week raises this every cycle for a week, and
     *       nothing is wrong.
     *   <li>{@link ManagedProjectRejectedError} means <i>no</i>. The awarding
     *       portal records the award as errored and stops asking. Raise it only
     *       when asking again cannot help - an unknown template, an expired end
     *       date, an allocation above what you will ever grant.
     * </ul>
     *
     * <p>A rejection where you meant "pending" strands an award that only needed
     * approving. A "pending" where you meant "rejected" leaves the caller
     * retrying forever against a decision that will never change.
     */
    private ProjectMapping answerForState(Award award) {
        if (Award.PENDING.equals(award.state())) {
            throw new ManagedProjectPendingError(reasonOr(award,
                    "awaiting approval by a site administrator"));
        }

        if (Award.REJECTED.equals(award.state())) {
            throw new ManagedProjectRejectedError(reasonOr(award, "this award was refused"));
        }

        // Approved once, but not attached to anything now - `removeAward`
        // severed it. "Pending", not "rejected": there is nothing wrong with the
        // award and an operator may attach it again, so the allocator should
        // keep asking rather than writing it off.
        if (!award.isAttached()) {
            throw new ManagedProjectPendingError(reasonOr(award,
                    "this award is not attached to a project"));
        }

        return mapping(award);
    }

    private static String reasonOr(Award award, String fallback) {
        String reason = award.reason();

        return reason == null || reason.isBlank() ? fallback : reason;
    }

    /**
     * A portal-level username from an email address.
     *
     * <p>{@link UserIdentifier} is {@code username.project.portal} and each part
     * is restricted to {@code [A-Za-z0-9_-]}, so the local part of the address
     * is sanitised rather than used raw.
     */
    static String username(String email) {
        String local = email.split("@", 2)[0];
        StringBuilder name = new StringBuilder();

        for (int i = 0; i < local.length(); i++) {
            char c = local.charAt(i);
            boolean ok = (c >= 'A' && c <= 'Z')
                    || (c >= 'a' && c <= 'z')
                    || (c >= '0' && c <= '9')
                    || c == '_'
                    || c == '-';

            name.append(ok ? c : '-');
        }

        // `-` cannot start an identifier component, and an address whose whole
        // local part sanitises away would leave nothing at all.
        while (name.length() > 0 && name.charAt(0) == '-') {
            name.deleteCharAt(0);
        }

        return name.length() == 0 ? "user" : name.toString();
    }

    // ----------------------------------------------------------------------
    // §4.1 Awards
    // ----------------------------------------------------------------------

    /**
     * {@code create_award <project_id> <AwardDetails JSON>} (arrives as
     * {@code create_project}).
     *
     * <p><b>This arrives repeatedly for awards you already hold.</b> The
     * awarding portal re-sends it every synchronisation cycle to re-assert the
     * award's state - waldur-mastermind's own comment reads "add it again just to
     * be sure". It is not asking for a second project. So: look it up, merge what
     * changed, and answer as you answered last time.
     */
    public ProjectMapping createAward(Job job) {
        Instruction instruction = job.instruction();
        String projectId = instruction.projectIdentifier(0).toString();
        AwardDetails details = instruction.awardDetails();

        // Which resource this award is for. It came in addressed to one of our
        // virtual agents, and that is the resource being asked for.
        String offering = offeringOf(job);

        if (details.template().isEmpty()) {
            throw new ManagedProjectRejectedError("no template named in the award");
        }

        String template = details.template().get().name();

        if (!templatesFor(offering).contains(template)) {
            throw new ManagedProjectRejectedError(
                    "template '" + template + "' is not offered on " + offering);
        }

        // **Is this an award at all?**
        //
        // An award is for a quantity, so an award of nothing is not an award:
        // there is no amount to provision against, nothing to enforce, and -
        // because the allocation is what names the unit - no way to say what any
        // usage we later report would mean. Refuse it rather than accepting an
        // award that cannot be honoured or reported.
        //
        // Terminal, like the template: the awarding portal has to send an
        // amount, and re-sending the same details without one will fail the same
        // way.
        Optional<Allocation> allocation = details.allocation()
                .filter(found -> !found.isEmpty());

        if (allocation.isEmpty()) {
            throw new ManagedProjectRejectedError("no allocation named in the award - an award"
                    + " has to be for some quantity, in units this site has agreed"
                    + " (e.g. '5000 GPUHR')");
        }

        Double size = allocation.get().size();

        if (size == null || !(size > 0.0)) {
            throw new ManagedProjectRejectedError("the allocation '" + allocation.get()
                    + "' awards nothing - an award has to be for some quantity");
        }

        // **Can we report usage for this award at all?**
        //
        // The allocation names the unit - "5000 GPUHR" - and that unit is what
        // every usage report for this award will be read in. If this resource
        // cannot express it, saying so now is the only honest answer: the
        // alternative is to accept the award and then answer every
        // `get_usage_report` with a well-formed zero.
        //
        // Terminal rather than pending, because no amount of asking again
        // changes what hardware we have.
        if (converterFor(offering, allocation.get()).isEmpty()) {
            String agreed = String.join(", ", conversionsFor(offering).keySet());

            throw new ManagedProjectRejectedError("no agreed conversion between '"
                    + allocation.get() + "' and this site's " + SITE_UNIT + " on " + offering
                    + " - it can hold awards in: " + (agreed.isEmpty() ? SITE_UNIT : agreed));
        }

        Optional<Award> held = store.award(offering, projectId);
        Award award;

        if (held.isEmpty()) {
            // New award *on this resource*. An award of the same name on another
            // offering is a different award and is left alone.
            award = store.create(offering, projectId, details.toJson(),
                    job.forwardedFor().map(Destination::toString).orElse(null));

            LOG.info("recorded new award " + projectId + " on " + offering
                    + ", awaiting approval");
        } else {
            award = held.get();

            // Known already: merge the incoming details over what we hold, so a
            // changed member list or end date takes effect. `merge` replaces
            // `members` and `allowed_domains` wholesale - they are definitive
            // sets owned by the awarding portal - while `notes` accumulate.
            award.setDetails(award.details().merge(details));

            // An award we previously detached, being asserted again. The
            // allocator still holds it, so it is asking us to attach it to a
            // project - which is a fresh decision for an operator, not something
            // to resurrect on the old project's behalf. Back to the pending
            // queue it goes, and the attachment history is left exactly as it
            // is: the days it owned before are still its days.
            if (Award.APPROVED.equals(award.state()) && !award.isAttached()) {
                award.setState(Award.PENDING);
                award.setReason("awaiting re-attachment to a project");

                LOG.info("award " + projectId + " on " + offering
                        + " was re-asserted after removal - pending again");
            }

            store.save(award);
        }

        return answerForState(award);
    }

    /**
     * {@code update_award <project_id> <AwardDetails JSON>} (arrives as
     * {@code update_project}).
     *
     * <p>An update for an award we have never seen is normal, not an error - a
     * missed message or a rebuilt database gets us here. Treat it as a create,
     * which routes it through the approval path rather than silently
     * provisioning something nobody approved.
     */
    public ProjectMapping updateAward(Job job) {
        String projectId = job.instruction().projectIdentifier(0).toString();
        String offering = offeringOf(job);

        if (store.award(offering, projectId).isEmpty()) {
            LOG.info("update for unknown award " + projectId + " on " + offering
                    + " - treating it as a create");
        }

        return createAward(job);
    }

    /**
     * {@code remove_award <project_id>} (arrives as {@code remove_project}).
     *
     * <p><b>Disconnects an award from a project. It does not delete the
     * project.</b> The answer is {@code <project_id>:None} - there is no longer
     * a project attached to name.
     *
     * <p>What removal actually ends is the award's claim on <i>future</i> days.
     * Billing is per-day, and a day belongs to whichever award the project was
     * last attached to during it, so: the day of removal still belongs to this
     * award, unless another award is attached later the same day, in which case
     * the whole day belongs to that one instead; and from the following day the
     * project bills to nothing until another award is attached.
     *
     * <p>So this <b>keeps the record and the usage figures</b> and only stamps
     * the detachment date. The days the award already owns still have to be
     * reportable - the allocator has not necessarily collected the final ones
     * yet, and it cannot ask a question we have destroyed the answer to.
     * Deleting would also make the month report as empty, and an empty report is
     * vacuously <i>complete</i>: we would be telling the allocator that nothing
     * was ever used and that the figure is final.
     *
     * <p>Removing an award we do not hold is <i>not</i> an error: the caller
     * wants it gone, and it is gone. A second removal of an award already
     * detached likewise does nothing - the first detachment date is the true
     * one, and moving it later would hand the award days it did not own.
     */
    public ProjectMapping removeAward(Job job) {
        String projectId = job.instruction().projectIdentifier(0).toString();
        String offering = offeringOf(job);

        // Only from this resource. An award of the same name on another offering
        // is a different award, and was not what the caller asked to remove.
        Optional<Award> award = store.award(offering, projectId);

        if (award.isPresent() && award.get().isAttached()) {
            store.detach(offering, projectId, LocalDate.now());
            LOG.info("detached award " + projectId + " on " + offering);
        }

        return ProjectMapping.parse(projectId + ":None");
    }

    /**
     * {@code get_award <project_id>} → {@link AwardDetails}.
     *
     * <p>How the awarding portal finds out what actually happened to an award it
     * created.
     *
     * <p><b>Populate {@code members} here.</b> This portal does not implement
     * {@code get_users}, and neither does waldur-mastermind - members travel with
     * the award instead, and this is the field callers read. They are already in
     * the stored details because that is how they arrived.
     */
    public AwardDetails getAward(Job job) {
        Award award = requireAward(job, job.instruction().projectIdentifier(0).toString());
        AwardDetails details = award.details();

        // A real portal overlays live project state here - the current member
        // list, the allocation as actually spent, the current end date - since
        // those may have moved on from what was last agreed.

        // `notes` is the one field that accumulates across a merge, which makes
        // it the right place for commentary that is not part of the award itself.
        if (!Award.APPROVED.equals(award.state())) {
            details.addNote(Note.of("portal", award.state() + ": " + award.reason()));
        }

        return details;
    }

    /** {@code get_awards <portal_id>} → every award that portal has made here. */
    public List<AwardDetails> getAwards(Job job) {
        List<AwardDetails> awards = new ArrayList<>();

        for (Award award : store.awardsOn(offeringOf(job), portalArgument(job))) {
            awards.add(award.details());
        }

        return awards;
    }

    /**
     * {@code get_projects <portal_id>} → a list of <b>mappings</b>, not details.
     *
     * <p>Easy to confuse with {@link #getAwards}; the return types are different
     * shapes. An award with no project attached maps to {@code :None}, the same
     * spelling {@link #removeAward} uses.
     *
     * <p>The test is {@code isAttached}, not the approval state. A detached
     * award is still <i>approved</i> - it was approved once and that did happen -
     * but it has no project attached now, so there is no identifier to put in a
     * mapping. Keying this on the state instead would build a mapping for an
     * award with nothing to map, and fail the whole listing rather than one
     * entry.
     */
    public List<ProjectMapping> getProjects(Job job) {
        List<ProjectMapping> mappings = new ArrayList<>();

        for (Award award : store.awardsOn(offeringOf(job), portalArgument(job))) {
            mappings.add(award.isAttached()
                    ? mapping(award)
                    : ProjectMapping.parse(award.projectId() + ":None"));
        }

        return mappings;
    }

    /** {@code get_project_mapping <project_id>} → that one award's mapping. */
    public ProjectMapping getProjectMapping(Job job) {
        return answerForState(
                requireAward(job, job.instruction().projectIdentifier(0).toString()));
    }

    // ----------------------------------------------------------------------
    // §4.3 Reports
    // ----------------------------------------------------------------------

    /**
     * Which award a project's usage on {@code date} is billed to, or empty for
     * nobody.
     *
     * <p>The rule is <i>the award the project was last attached to on that
     * day</i>, and every part of that sentence is doing work:
     *
     * <ul>
     *   <li><b>"last attached"</b> - if two awards were attached during the day,
     *       the later attachment takes the whole day, not just the part after
     *       the handover. Usage is accounted per day, so a day is indivisible;
     *       splitting it would need per-hour attribution that neither side
     *       keeps.
     *   <li><b>"on that day"</b> - an award detached <i>during</i> the day was
     *       attached during it, so it stays a candidate. It stops being one from
     *       the next day on, which is why removal takes effect at most the day
     *       after.
     *   <li><b>nobody</b> - a day on which the project was attached to nothing
     *       is billed to nothing. The usage is real and stays in our own
     *       accounting; there is simply no award for it to appear under, so it
     *       appears in no report.
     * </ul>
     *
     * <p>A consequence worth being explicit about: <b>a day's attribution is not
     * settled until the day is over.</b> Attaching an award this afternoon
     * changes who owns this morning. That is the deeper reason completeness is a
     * decision rather than a calendar comparison.
     */
    public static Optional<Award> ownerOfDay(
            List<Award> awards, String localProjectId, LocalDate date) {
        Award owner = null;
        LocalDate ownerSince = null;

        for (Award award : awards) {
            for (Attachment attachment : award.attachments()) {
                if (!attachment.project().equals(localProjectId)) {
                    // The same award may have been attached to several projects
                    // of ours over its life. Only this project's episodes bill
                    // here.
                    continue;
                }

                if (!attachment.covers(date)) {
                    continue;
                }

                if (ownerSince == null || attachment.since().isAfter(ownerSince)) {
                    owner = award;
                    ownerSince = attachment.since();
                }
            }
        }

        return Optional.ofNullable(owner);
    }

    /**
     * Whether {@code award} was attached to {@code localProjectId} at any point
     * during {@code month}.
     *
     * <p>Used to decide whether a month with no figures deserves an "incomplete,
     * ask again" placeholder. A month wholly outside every attachment this award
     * had on this project is a different case: the award owned nothing then and
     * never will, so an empty and therefore complete answer is the truth rather
     * than an accident.
     */
    private static boolean couldOwnAnyOf(
            Award award, String localProjectId, DateRange month) {
        for (Attachment attachment : award.attachments()) {
            if (!attachment.project().equals(localProjectId)) {
                continue;
            }

            if (attachment.since().isAfter(month.endDate())) {
                continue;
            }

            Optional<LocalDate> to = attachment.to();

            if (to.isEmpty() || !to.get().isBefore(month.startDate())) {
                return true;
            }
        }

        return false;
    }

    /**
     * Assemble a {@link ProjectUsageReport} from the figures we hold.
     *
     * <p>Two things are going on, and the second is the interesting one.
     *
     * <p>Nothing here hand-assembles JSON: construct the report, add a daily
     * report per date, and the type handles the wire format.
     *
     * <p>More importantly, <b>the figures are recorded in our namespace and
     * asked for in theirs</b>. Our accounting produces usage for
     * {@code myproject1.site}; the awarding portal asked about
     * {@code myaward1.allocator}. So the report is built against our own project
     * identifier and then remapped into theirs at the end. That translation is
     * only possible because approving the award fixed the mapping between the
     * two.
     *
     * <p>The report is also scoped to one resource. A project lives on the
     * offering its award was created through, so asking a different offering
     * about it returns an empty report rather than an error.
     *
     * <p><b>On {@code is_complete}.</b> The allocator asks per month, and a
     * report that comes back complete is one it will not ask for again. That
     * makes completeness a claim about the future - "these figures will not
     * change" - which only the site's operations team can make, so here it is
     * driven by {@link LocalProject#finalMonths} rather than inferred from the
     * calendar.
     */
    public ProjectUsageReport buildUsageReport(
            String offering, String projectId, DateRange dateRange) {
        ProjectUsageReport empty =
                new ProjectUsageReport(ProjectIdentifier.parse(projectId));

        // **The project may simply not be on this resource.** An awarding portal
        // holding an award on `cluster1` can perfectly well ask `cluster2` about
        // it - the identifier is the same, and nothing stops the question. The
        // honest answer is an empty report: nothing was used here, because the
        // project is not here. An error would say "something is broken", which
        // is not true, and would fail a caller that is simply sweeping every
        // offering it knows about.
        Optional<Award> found = store.award(offering, projectId);

        // Never attached to anything of ours, so there are no figures to find.
        // Note the test is the award's *history*, not whether it is attached
        // now: a removed award still owns the days it was attached for, and
        // refusing to report them - or reporting them as an empty, and therefore
        // vacuously complete, month - is how the final days of an award get
        // silently lost.
        if (found.isEmpty() || found.get().projectsEverAttached().isEmpty()) {
            return empty;
        }

        Award award = found.get();
        List<String> everAttached = award.projectsEverAttached();

        // The identifier the answer is expressed in. Almost always the award's
        // one and only project; the most recent one if an operator has moved the
        // award between projects. Everything is remapped into the *awarding*
        // portal's namespace at the end, so this choice affects only the
        // intermediate form.
        ProjectIdentifier localProject =
                ProjectIdentifier.parse(everAttached.get(everAttached.size() - 1));

        // **The unit every figure below is expressed in.** Our accounting
        // produced them in `SITE_UNIT`; the award was allocated in whatever the
        // awarding portal chose, and that is what its reports mean. Read from
        // the award we hold rather than from the request, because the request
        // does not carry it.
        //
        // This cannot be empty for an award we accepted: `createAward` refuses
        // an award with no allocation, and one whose unit we have no agreed
        // factor for. A record predating those checks would otherwise crash a
        // report, though, and a report is not the place to discover it - so it
        // is logged loudly and the figures go out in our own unit, which is at
        // least a number we can name.
        Allocation allocation = award.details().allocation().orElse(null);
        DoubleFunction<Usage> convert = converterFor(offering, allocation).orElse(null);

        if (convert == null) {
            LOG.log(Level.SEVERE, "award " + projectId + " on " + offering
                    + " is allocated in '" + allocation + "', which this site cannot account"
                    + " in - reporting our own " + SITE_UNIT + " unconverted");

            convert = Usage::fromHours;
        }

        ProjectUsageReport report = new ProjectUsageReport(localProject);
        java.util.Set<String> monthsWithDays = new TreeSet<>();
        java.util.Set<String> settled = new TreeSet<>();

        // The figures are the *project's*, and this award only claims days of
        // them. Every award ever attached to the same project is needed to work
        // out which days, because "the award last attached that day" is a
        // question about the whole attachment history and not about this award
        // alone.
        for (String localId : everAttached) {
            LocalProject project = store.project(localId);
            List<Award> siblings = store.awardsForLocalProject(localId);
            settled.addAll(project.finalMonths());

            for (Map.Entry<LocalDate, Map<String, Double>> day : project.usage().entrySet()) {
                LocalDate date = day.getKey();

                // **Whose day is this?** A day of this project's usage is billed
                // to the award it was last attached to during that day - which
                // may be a different award than the one being asked about, or
                // none at all if the project was unattached then. Either way it
                // is not ours to report, and reporting it anyway would bill it
                // twice.
                Optional<Award> owner = ownerOfDay(siblings, localId, date);

                if (owner.isEmpty() || !owner.get().key().equals(award.key())) {
                    continue;
                }

                DailyProjectUsageReport daily = new DailyProjectUsageReport();

                for (Map.Entry<String, Double> perUser : day.getValue().entrySet()) {
                    String email = perUser.getKey();

                    // The mapping records which portal user each local name
                    // belongs to. At the portal layer the local name is the
                    // member's email.
                    UserIdentifier user = UserIdentifier.parse(
                            username(email) + "." + localProject);
                    report.addMapping(UserMapping.parse(
                            user + ":" + email + ":" + localProject));

                    // ...and the figure is converted out of our unit into the
                    // one the award was allocated in. `createAward` refused any
                    // award we could not do this for.
                    daily.addUsage(email, convert.apply(perUser.getValue()));
                }

                // Completeness is a *decision*, not a date comparison. A day is
                // reported complete only when the site has declared its month
                // final. Guessing from the calendar ("the day has passed, so it
                // must be settled") claims the figures will not change, which
                // nobody but the operations team can know.
                String month = monthKey(date);
                monthsWithDays.add(month);

                if (settled.contains(month)) {
                    daily.setComplete();
                }

                report.setReport(date, daily);
            }
        }

        // A month this award could still own days in, but that we have no data
        // for, needs an explicit, incomplete placeholder.
        //
        // `ProjectUsageReport.isComplete` is "every day I contain is complete",
        // which is vacuously **true** for a report containing no days at all. So
        // a month we have simply not ingested yet would otherwise answer
        // "nothing was used, and that is final" - and the allocator would
        // believe it and stop asking. A zero-usage day that is *not* marked
        // complete says the honest thing instead: nothing so far, ask again.
        for (DateRange monthRange : dateRange.months()) {
            String month = monthKey(monthRange.startDate());

            if (settled.contains(month) || monthsWithDays.contains(month)) {
                continue;
            }

            // A month wholly outside this award's attachment window is a
            // different case, and an empty report for it is *correct*: this
            // award owned nothing then and never will, so "nothing, and that is
            // final" is the truth rather than an accident. Only months the award
            // could still be billed days in get a placeholder.
            boolean relevant = false;

            for (String localId : everAttached) {
                if (couldOwnAnyOf(award, localId, monthRange)) {
                    relevant = true;
                    break;
                }
            }

            if (!relevant) {
                continue;
            }

            // `months()` yields whole calendar months, so a month's first day
            // can fall before the range that was actually asked about - and the
            // `filter` at the end would drop the placeholder again, putting the
            // vacuous-complete answer straight back. Anchor it inside the
            // requested range instead.
            LocalDate anchor = monthRange.startDate().isBefore(dateRange.startDate())
                    ? dateRange.startDate()
                    : monthRange.startDate();

            report.setReport(anchor, new DailyProjectUsageReport());
        }

        // Now translate the whole report into the awarding portal's namespace.
        // This is the mapping being used: they asked about
        // `myaward1.allocator`, so that is what the answer must be about.
        // `remapProject` rewrites the project and rebuilds every
        // `UserIdentifier` with it, turning `alice.myproject1.site` into
        // `alice.myaward1.allocator` - the member's email is unchanged, because
        // that is the same person either way.
        return report.remapProject(ProjectIdentifier.parse(projectId)).filter(dateRange);
    }

    /** {@code "YYYY-MM"} - how a month is named in {@link LocalProject#finalMonths}. */
    static String monthKey(LocalDate date) {
        return String.format(Locale.ROOT, "%04d-%02d", date.getYear(), date.getMonthValue());
    }

    /**
     * The {@link DateRange} argument, which the grammar fills in as
     * {@code this_week} when the caller omits it - so in practice it is always
     * there.
     */
    private static DateRange dateRangeOf(Job job, int index) {
        List<String> arguments = job.instruction().arguments();

        if (arguments.size() > index) {
            return DateRange.parse(arguments.get(index));
        }

        return DateRange.thisWeek();
    }

    /**
     * {@code get_usage_report <project_id> <DateRange>} →
     * {@link ProjectUsageReport}.
     *
     * <p><b>Answer from cache.</b> You have about 30 seconds before the caller
     * gives up - not the two minutes the job expiry suggests. So this reads
     * figures pushed in earlier rather than going away to compute them. If your
     * accounting takes minutes, serve what you have and let the next request
     * collect the fresher numbers; there will be a next request, because callers
     * retry.
     */
    public ProjectUsageReport getUsageReport(Job job) {
        return buildUsageReport(
                offeringOf(job),
                job.instruction().projectIdentifier(0).toString(),
                dateRangeOf(job, 1));
    }

    /**
     * {@code get_usage_reports <portal_id>} → the portal-level roll-up.
     *
     * <p>A loop over the per-project path. Each single-project report is lifted
     * into the portal-level shape so they can be combined.
     */
    public UsageReport getUsageReports(Job job) {
        String portal = portalArgument(job);
        String offering = offeringOf(job);
        DateRange dateRange = dateRangeOf(job, 1);

        // Only the awards on this resource, so a portal-level roll-up asked of
        // `cluster2` covers `cluster2` and nothing else.
        List<UsageReport> reports = new ArrayList<>();

        for (Award award : store.awardsOn(offering, portal)) {
            reports.add(buildUsageReport(offering, award.projectId(), dateRange)
                    .toUsageReport());
        }

        // `combine` needs at least one report, so an empty portal answers with
        // an empty report rather than failing.
        if (reports.isEmpty()) {
            return new UsageReport(new PortalIdentifier(portal));
        }

        return UsageReport.combine(reports);
    }

    /**
     * {@code get_storage_report <project_id> <DateRange>} →
     * {@link ProjectStorageReport}.
     *
     * <p>This portal has no storage to report, and answers with an <b>empty
     * report</b> rather than an error. Empty says "nothing here"; an error says
     * "something is broken", and only the first is true. That is the same answer
     * a project which is not on this resource gets, for the same reason.
     */
    public ProjectStorageReport getStorageReport(Job job) {
        return new ProjectStorageReport(job.instruction().projectIdentifier(0));
    }

    /** {@code get_storage_reports <portal_id>} → an empty portal-level roll-up. */
    public StorageReport getStorageReports(Job job) {
        return new StorageReport(new PortalIdentifier(portalArgument(job)));
    }

    /**
     * The portal-name argument, which is a bare name rather than a dotted
     * identifier.
     */
    private static String portalArgument(Job job) {
        return new PortalIdentifier(job.instruction().argument(0)).portal();
    }

    // ----------------------------------------------------------------------
    // Dispatch
    // ----------------------------------------------------------------------

    /** What one instruction's handler does. */
    private interface Handler {
        OpenPortalType handle(Job job);
    }

    /**
     * A handler answering a list, which the wire carries as a JSON array.
     *
     * <p>The element type is declared alongside the handler rather than read
     * off the first value, because an empty list has no first value - and
     * {@code get_projects} answering an empty list still has to say it was a
     * list of mappings.
     */
    private record ListHandler(
            String elementType, java.util.function.Function<Job, List<? extends OpenPortalType>> fn) {

        List<? extends OpenPortalType> handle(Job job) {
            return fn.apply(job);
        }

        /**
         * The {@code result_type} a list goes on the wire as: {@code Vec<T>},
         * not {@code T}.
         *
         * <p>The Rust side names a {@code Vec<T>} result exactly that way, and
         * the awarding portal deserialises the JSON against the name it was
         * given - so answering {@code get_projects} with a bare
         * {@code "ProjectMapping"} is a different answer from a list of one.
         */
        String typeName() {
            return "Vec<" + elementType + ">";
        }
    }

    private final Map<String, Handler> handlers = handlers();
    private final Map<String, ListHandler> listHandlers = listHandlers();

    /**
     * Every instruction this portal answers, keyed on the command name as it
     * arrives.
     *
     * <p>Anything absent is answered with
     * {@link OpenPortalUnsupportedCommandError}, which is a legitimate answer: a
     * portal implements as much of the contract as it has answers for.
     * {@code get_users} is deliberately absent - members travel in
     * {@code AwardDetails.members} instead.
     *
     * <p><b>Both spellings of the award instructions are here, deliberately.</b>
     * An awarding portal sends {@code create_award}; the agents currently
     * deliver it under its original name, {@code create_project}, and that is
     * what you see in a job's {@code command} field today. The wire vocabulary
     * is moving to the {@code *_award} spellings (and the attach/detach pair may
     * end up named for what they actually do) before 1.0, so a table keyed on
     * only one of the two will start answering
     * {@code OpenPortalUnsupportedCommandError} on the day it changes. Keying on
     * both costs three entries and spans the change - and since each pair is one
     * instruction under two names, they share a handler rather than duplicating
     * it.
     */
    private Map<String, Handler> handlers() {
        Map<String, Handler> table = new LinkedHashMap<>();

        // Attaching an award to a project, and detaching it again.
        table.put("create_award", this::createAward);
        table.put("create_project", this::createAward);
        table.put("update_award", this::updateAward);
        table.put("update_project", this::updateAward);
        table.put("remove_award", this::removeAward);
        table.put("remove_project", this::removeAward);

        // Reading awards back.
        table.put("get_award", this::getAward);
        table.put("get_project", this::getAward);
        table.put("get_project_mapping", this::getProjectMapping);

        // Accounting.
        table.put("get_usage_report", this::getUsageReport);
        table.put("get_usage_reports", this::getUsageReports);
        table.put("get_storage_report", this::getStorageReport);
        table.put("get_storage_reports", this::getStorageReports);

        return table;
    }

    /** The two that answer a list rather than a single value. */
    private Map<String, ListHandler> listHandlers() {
        Map<String, ListHandler> table = new LinkedHashMap<>();

        table.put("get_awards", new ListHandler("ProjectDetails", this::getAwards));
        table.put("get_projects", new ListHandler("ProjectMapping", this::getProjects));

        return table;
    }

    /** Every command this portal will answer. */
    public java.util.Set<String> commands() {
        java.util.Set<String> commands = new TreeSet<>(handlers.keySet());
        commands.addAll(listHandlers.keySet());

        return commands;
    }

    /**
     * Check the request came in through an offering we actually advertise.
     *
     * <p>Note what this is <i>not</i> doing. It is not deciding whether the
     * caller may see a particular award - the offering is not a permission, it
     * is which resource is being talked about, and every handler scopes itself by
     * it via {@link #offeringOf}. This only refuses a name we do not offer at
     * all, which should never happen: the portal agent only forwards requests for
     * offerings we registered. It is here as a backstop, not as the access
     * control.
     *
     * <p>{@code forwarded_for} is set by our own portal agent and not by the
     * caller, which is why it is the field worth trusting. Its first element is
     * the portal that asked; its last is the offering they came in through.
     */
    private void authorise(Job job) {
        String offering = offeringOf(job);

        if (!offeringNames().contains(offering)) {
            throw new ManagedProjectRejectedError(
                    "offering '" + offering + "' is not advertised by this portal");
        }
    }

    /**
     * Run one job and return the answered job, without sending it.
     *
     * <p>Split out from {@link #handle} so it can be tested without a bridge -
     * see {@code SitePortalTest}, which drives every handler through here.
     */
    public Job answer(Job job) {
        String command = job.instruction().command();

        try {
            Handler handler = handlers.get(command);
            ListHandler listHandler = listHandlers.get(command);

            if (handler == null && listHandler == null) {
                throw new OpenPortalUnsupportedCommandError(
                        "this portal does not implement '" + command + "'");
            }

            authorise(job);

            if (listHandler != null) {
                List<? extends OpenPortalType> values = listHandler.handle(job);

                LOG.info(command + " " + job.id() + " -> " + values.size() + " "
                        + listHandler.typeName());

                return job.completed(list(values), listHandler.typeName());
            }

            OpenPortalType result = handler.handle(job);

            LOG.info(command + " " + job.id() + " -> " + result.typeName());

            return job.completed(result);

        } catch (OpenPortalError e) {
            // An expected failure, already the right class. `errored` encodes
            // the class into the message so the awarding portal recovers it.
            LOG.info(command + " " + job.id() + " -> " + e.wireClass());

            return job.errored(e);

        } catch (RuntimeException e) {
            // A bug in this portal. Still answered, so the caller learns that
            // something went wrong instead of waiting for the job to expire.
            LOG.log(Level.SEVERE, "unhandled error in " + command, e);

            return job.errored(new OpenPortalError("internal error: " + e));
        }
    }

    /**
     * A list of results as the wire carries one: a JSON array, under the
     * element type's name.
     *
     * <p>The array is the whole result; the type name that goes with it is
     * {@link ListHandler#typeName}.
     */
    private static com.fasterxml.jackson.databind.JsonNode list(
            List<? extends OpenPortalType> values) {
        var array = org.openportal.Json.array();
        values.forEach(value -> array.add(value.toJson()));

        return array;
    }

    /** Run one job and post its result through {@code bridge}. Always posts something. */
    public void handle(org.openportal.BridgeClient bridge, Job job) {
        send(bridge, answer(job));
    }

    /**
     * Post a result, retrying a few times.
     *
     * <p>A failure here loses the answer entirely, so it is worth more than one
     * attempt - waldur-mastermind retries five times at one-second intervals.
     */
    private static void send(org.openportal.BridgeClient bridge, Job job) {
        for (int attempt = 1; attempt <= 5; attempt++) {
            try {
                bridge.sendResult(job);

                return;
            } catch (RuntimeException e) {
                LOG.warning("sendResult failed (attempt " + attempt + "/5): " + e.getMessage());

                try {
                    Thread.sleep(1000);
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();

                    break;
                }
            }
        }

        LOG.severe("gave up sending the result for job " + job.id());
    }
}
