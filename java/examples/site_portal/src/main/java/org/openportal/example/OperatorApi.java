// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;
import com.sun.net.httpserver.HttpExchange;
import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.time.LocalDate;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import java.util.concurrent.Executors;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.openportal.Allocation;
import org.openportal.AwardDetails;
import org.openportal.DailyProjectUsageReport;
import org.openportal.Json;
import org.openportal.Notification;
import org.openportal.ProjectIdentifier;
import org.openportal.ProjectUsageReport;
import org.openportal.UserIdentifier;

/**
 * The HTTP surface: two endpoints OpenPortal calls, and the rest an operator
 * calls.
 *
 * <p>Split down the middle, and the middle is worth seeing. The
 * {@code /signal/*} pair is <b>the contract</b> - the bridge calls them and
 * their behaviour is specified. Everything else is <b>this site's own</b>: no
 * part of OpenPortal knows or cares that approving an award is a
 * {@code POST /awards/.../approve}, and a real portal would put approval behind
 * whatever admin interface it already has.
 *
 * <p>Served by {@code com.sun.net.httpserver}, which ships with the JDK, so the
 * example has no framework dependency and the code that matters is not hidden
 * behind annotations. A real portal would use whatever it already uses.
 *
 * <p><b>There is no authentication here.</b> The Python example is the same, for
 * the same reason: this is bound to localhost and the point is the contract, not
 * the admin interface. Do not expose it.
 */
public final class OperatorApi {

    private static final Logger LOG = Logger.getLogger(OperatorApi.class.getName());

    private final App app;
    private final SitePortal portal;
    private final Store store;
    private HttpServer server;

    OperatorApi(App app) {
        this.app = app;
        this.portal = app.portal();
        this.store = app.portal().store();
    }

    /** Start listening. Returns the port, which is the one asked for. */
    int start(int port) throws IOException {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", port), 0);

        // A small fixed pool rather than a thread per request: the handlers are
        // short and the signal endpoint hands its work off anyway.
        server.setExecutor(Executors.newFixedThreadPool(4));
        server.createContext("/", this::route);
        server.start();

        return server.getAddress().getPort();
    }

    void stop() {
        if (server != null) {
            server.stop(0);
        }
    }

    // ----------------------------------------------------------------------
    // Routing
    // ----------------------------------------------------------------------

    private void route(HttpExchange exchange) throws IOException {
        String method = exchange.getRequestMethod();
        String path = exchange.getRequestURI().getPath();
        List<String> parts = segments(path);

        try {
            JsonNode answer = dispatch(method, parts, exchange);

            respond(exchange, 200, answer);

        } catch (HttpError e) {
            respond(exchange, e.status(), detail(e.getMessage()));

        } catch (IllegalArgumentException e) {
            // A malformed identifier, template or offering name. The operator
            // typed it, so the message is worth showing them.
            respond(exchange, 400, detail(e.getMessage()));

        } catch (RuntimeException e) {
            LOG.log(Level.SEVERE, method + " " + path + " failed", e);
            respond(exchange, 500, detail("internal error: " + e));
        }
    }

    private JsonNode dispatch(String method, List<String> parts, HttpExchange exchange)
            throws IOException {
        // What OpenPortal calls.
        if (parts.equals(List.of("signal", "job")) && method.equals("GET")) {
            return signalJob(query(exchange).get("job_id"));
        }

        if (parts.equals(List.of("signal", "notification")) && method.equals("GET")) {
            return signalNotification(query(exchange).get("notification_id"));
        }

        // What an operator calls.
        if (parts.equals(List.of("health")) && method.equals("GET")) {
            return health();
        }

        if (parts.equals(List.of("offerings"))) {
            if (method.equals("GET")) {
                return listOfferings();
            }

            if (method.equals("POST")) {
                return addOffering(body(exchange));
            }
        }

        if (parts.equals(List.of("offerings", "sync")) && method.equals("POST")) {
            return resyncOfferings();
        }

        if (parts.size() == 2 && parts.get(0).equals("offerings") && method.equals("DELETE")) {
            return removeOffering(parts.get(1));
        }

        if (parts.equals(List.of("awards")) && method.equals("GET")) {
            return listAwards();
        }

        if (parts.size() == 3 && parts.get(0).equals("awards") && method.equals("GET")) {
            return oneAward(parts.get(1), parts.get(2));
        }

        if (parts.size() == 4 && parts.get(0).equals("awards") && method.equals("POST")) {
            if (parts.get(3).equals("approve")) {
                return approve(parts.get(1), parts.get(2), body(exchange));
            }

            if (parts.get(3).equals("reject")) {
                return reject(parts.get(1), parts.get(2), body(exchange));
            }
        }

        if (parts.size() == 3
                && parts.get(0).equals("projects")
                && parts.get(2).equals("usage")
                && method.equals("PUT")) {
            return pushUsage(parts.get(1), body(exchange));
        }

        if (parts.size() == 4
                && parts.get(0).equals("projects")
                && parts.get(2).equals("usage")
                && parts.get(3).equals("finalise")
                && method.equals("POST")) {
            return finaliseUsage(parts.get(1), body(exchange));
        }

        throw new HttpError(404, "no such endpoint: " + method + " /" + String.join("/", parts));
    }

    // ----------------------------------------------------------------------
    // What OpenPortal calls
    // ----------------------------------------------------------------------

    /**
     * {@code GET /signal/job?job_id=<uuid>} - the bridge telling us a job has
     * arrived.
     *
     * <p>Three things this endpoint gets right, all of which matter:
     *
     * <p><b>It returns immediately.</b> The job is queued and 200 goes back at
     * once; the work happens afterwards. The bridge retries a failed signal five
     * times at two-second intervals and then <i>removes the job from the board
     * and errors it</i>, so a slow signal endpoint fails requests outright.
     *
     * <p><b>It treats the job id as a secret.</b> The endpoint has no credential
     * of its own; the id is a random UUID known only to the bridge and to us. An
     * id the bridge does not have is not a request to act on, so it gets a 403
     * rather than being fetched.
     *
     * <p><b>It de-duplicates.</b> The same id can arrive more than once, and the
     * work must not happen twice.
     */
    private JsonNode signalJob(String jobId) {
        if (jobId == null || jobId.isBlank()) {
            throw new HttpError(400, "job_id is required");
        }

        java.util.UUID id;

        try {
            id = java.util.UUID.fromString(jobId);
        } catch (IllegalArgumentException e) {
            throw new HttpError(400, "job_id is not a UUID");
        }

        if (!app.claim(id)) {
            LOG.info("job " + id + " already handled - ignoring duplicate signal");

            return Json.object();
        }

        org.openportal.Job job;

        try {
            job = app.bridge().fetchJob(id);
        } catch (RuntimeException e) {
            // The bridge does not know this id. Either it has already been
            // dealt with, or somebody is guessing.
            app.release(id);

            throw new HttpError(403, "no such job");
        }

        app.background(() -> portal.handle(app.bridge(), job));

        return Json.object();
    }

    /**
     * {@code GET /signal/notification?notification_id=<uuid>} - a
     * fire-and-forget event.
     *
     * <p>Notifications are pull-model, so the body is never posted to an
     * unauthenticated endpoint: we are told an id, we fetch it, we return 200.
     * There is nothing to answer.
     *
     * <p>Make the handling idempotent - the same notification can be delivered
     * more than once if a retry races a successful fetch.
     */
    private JsonNode signalNotification(String notificationId) {
        if (notificationId == null || notificationId.isBlank()) {
            throw new HttpError(400, "notification_id is required");
        }

        Notification notification;

        try {
            notification = app.bridge()
                    .fetchNotification(java.util.UUID.fromString(notificationId));
        } catch (RuntimeException e) {
            // Already collected, or unknown. Either way there is nothing to do,
            // and 200 stops the bridge retrying.
            return Json.object();
        }

        LOG.info("notification " + notificationId + ": " + notification.eventType()
                + " " + notification.eventArgument());

        // A real portal would act on these - `user_added`, `award_changed`, and
        // so on. See notification-protocol.md for the vocabulary.

        return Json.object();
    }

    // ----------------------------------------------------------------------
    // What an operator calls - not part of any OpenPortal contract
    // ----------------------------------------------------------------------

    private JsonNode health() {
        ObjectNode answer = Json.object();
        answer.put("status", "ok");
        answer.put("portal", app.myPortal());

        ArrayNode offerings = answer.putArray("offerings");
        portal.offeringNames().forEach(offerings::add);

        answer.put("awards", store.allAwards().size());

        return answer;
    }

    /**
     * {@code GET /offerings} - every resource we advertise.
     *
     * <p>Two sources, deliberately: our own state, and {@code registered} on
     * each row, which is what the OpenPortal agents actually hold right now. The
     * two differing means a sync did not happen or did not take.
     */
    private JsonNode listOfferings() {
        java.util.Set<String> registered = app.registeredDestinations();
        List<Award> awards = store.allAwards();
        ArrayNode rows = Json.array();

        for (Offering offering : portal.offerings()) {
            rows.add(offeringJson(offering, registered, awards));
        }

        return rows;
    }

    /** One offering as the endpoints here report it. */
    private JsonNode offeringJson(
            Offering offering, java.util.Set<String> registered, List<Award> awards) {
        ObjectNode row = Json.object();
        row.put("name", offering.name());

        ArrayNode templates = row.putArray("templates");
        offering.templates().forEach(templates::add);

        row.put("since", offering.since().map(LocalDate::toString).orElse(null));

        // What an award on this resource may be allocated in, and what one of
        // our units is worth in each. Our own unit is always here at 1.0; a unit
        // absent from it is one an award cannot be held in, and `createAward`
        // refuses those rather than guessing a factor.
        row.put("site_unit", SitePortal.SITE_UNIT);

        ObjectNode conversions = row.putObject("conversions");
        portal.conversionsFor(offering.name()).forEach(conversions::put);

        // How many awards this resource holds. Shown because it is what makes
        // withdrawing one consequential: those awards stay on record and stop
        // being reachable, rather than being deleted.
        int held = 0;

        for (Award award : awards) {
            if (award.offering().equals(offering.name())) {
                held++;
            }
        }

        row.put("awards", held);

        // What the awarding portals address, and whether OpenPortal currently
        // has it registered. The second is the agents' view rather than ours.
        List<String> destinations = app.destinationsFor(offering.name());
        ArrayNode paths = row.putArray("destinations");
        boolean allRegistered = true;

        for (String destination : destinations) {
            paths.add(destination);

            if (!registered.contains(destination)) {
                allRegistered = false;
            }
        }

        row.put("registered", allRegistered);

        return row;
    }

    /**
     * {@code POST /offerings} - start advertising a resource.
     *
     * <p>{@code name} is the resource's own name - {@code cluster1}, not
     * {@code cluster1.site.allocator}. The other two elements are added from our
     * own portal name and the list of awarding portals, because neither is an
     * operator's to choose here: an offering in somebody else's namespace is not
     * something this portal can advertise.
     *
     * <p>{@code templates} is <b>required</b>: what a resource can be asked for
     * is the site's decision about that resource, and defaulting it would
     * publish a guess under the site's name that an awarding portal could not
     * tell from a policy.
     *
     * <p>{@code conversions} is what the two portals agreed each of this site's
     * units is worth in an awarding portal's: {@code {"GPUHR": 4}} means one
     * node hour here is four of their GPU hours. Optional - without it the
     * resource can only hold awards allocated in this site's own unit - and
     * omitting it on a later call keeps what was already agreed, so the
     * templates can be changed on their own.
     */
    private JsonNode addOffering(JsonNode body) {
        String name = text(body, "name");
        List<String> templates = new ArrayList<>();
        body.path("templates").forEach(entry -> templates.add(entry.asText()));

        Map<String, Double> conversions = null;

        if (body.hasNonNull("conversions")) {
            conversions = new TreeMap<>();
            Map<String, Double> agreed = conversions;
            body.get("conversions").fields().forEachRemaining(entry ->
                    agreed.put(entry.getKey(), entry.getValue().asDouble()));
        }

        Offering offering = portal.addOffering(name, templates, conversions);

        // **Until an offering is registered, requests for it have nowhere to
        // land** - they are held and only delivered once it exists. So the set
        // is republished on every change.
        java.util.Set<String> registered = app.publishOfferings();

        return offeringJson(offering, registered, store.allAwards());
    }

    /**
     * {@code DELETE /offerings/{name}} - stop advertising a resource.
     *
     * <p>The awards on it are kept - see {@link Store#removeOffering}.
     */
    private JsonNode removeOffering(String name) {
        Optional<Offering> removed = portal.removeOffering(name);

        if (removed.isEmpty()) {
            throw new HttpError(404, "not offering '" + name + "'");
        }

        java.util.Set<String> registered = app.publishOfferings();

        ObjectNode answer = Json.object();
        answer.put("removed", name);

        ArrayNode remaining = answer.putArray("offerings");
        portal.offeringNames().forEach(remaining::add);

        ArrayNode active = answer.putArray("registered");
        registered.forEach(active::add);

        // The awards that just became unreachable. Worth reporting, because
        // "withdrawn" and "deleted" are different things.
        int stranded = 0;

        for (Award award : store.allAwards()) {
            if (award.offering().equals(name)) {
                stranded++;
            }
        }

        answer.put("awards_kept", stranded);

        return answer;
    }

    /**
     * {@code POST /offerings/sync} - re-register the set with OpenPortal.
     *
     * <p>Nothing here changes what we offer. It is for the case where the
     * agents' view and ours have diverged - a portal agent restarted, a sync
     * failed - and {@code registered} on a listing row is showing false.
     */
    private JsonNode resyncOfferings() {
        java.util.Set<String> registered = app.publishOfferings();

        ObjectNode answer = Json.object();

        ArrayNode ours = answer.putArray("offerings");
        portal.offeringNames().forEach(ours::add);

        ArrayNode active = answer.putArray("registered");
        registered.forEach(active::add);

        return answer;
    }

    /** {@code GET /awards} - every award we hold, with its approval state. */
    private JsonNode listAwards() {
        ArrayNode rows = Json.array();

        for (Award award : store.allAwards()) {
            AwardDetails details = award.details();
            ObjectNode row = Json.object();

            row.put("offering", award.offering());
            row.put("project_id", award.projectId());
            row.put("state", award.state());
            row.put("reason", award.reason());
            row.put("local_project_id", award.localProjectId().orElse(null));
            row.put("name", details.name().orElse(null));
            row.put("template", details.template().map(Object::toString).orElse(null));

            // The award as the awarding portal expressed it, and what it is
            // worth in our own unit at the agreed factor - which is the number
            // this site would actually enforce a quota against.
            Optional<Allocation> allocation = details.allocation();
            row.put("allocation", allocation.map(Object::toString).orElse(null));
            row.put("allocation_in_site_units",
                    portal.toSiteUnits(award.offering(), allocation.orElse(null)).orElse(null));

            ArrayNode members = row.putArray("members");
            details.members().ifPresent(found -> found.keySet().forEach(members::add));

            // Whether it is attached now, and the full attachment history. A
            // detached award is kept rather than deleted: it still owns the days
            // it was attached for, and those still have to be reportable, so the
            // operator needs to see them.
            row.put("attached", award.isAttached());

            ArrayNode attachments = row.putArray("attachments");
            award.attachments().forEach(attachment -> attachments.add(attachment.json()));

            // Which months the site has declared final. A property of the
            // project rather than of the award, so it is looked up against the
            // project this award was *last* attached to, which a detached award
            // still has.
            ArrayNode settled = row.putArray("final_months");
            List<String> everAttached = award.projectsEverAttached();

            if (!everAttached.isEmpty()) {
                store.project(everAttached.get(everAttached.size() - 1))
                        .finalMonths()
                        .forEach(settled::add);
            }

            rows.add(row);
        }

        return rows;
    }

    /**
     * {@code GET /awards/{offering}/{project_id}} - one award.
     *
     * <p>Keyed on the resource as well as the identifier, because that pair is
     * what identifies an award - the same name on another resource is a
     * different award.
     */
    private JsonNode oneAward(String offering, String projectId) {
        return store.award(offering, projectId)
                .orElseThrow(() -> new HttpError(404, "no such award on that offering"))
                .json();
    }

    /**
     * {@code POST /awards/{offering}/{project_id}/approve} - approve an award,
     * and <b>give it its identifier here</b>.
     *
     * <p>This is the moment the mapping is made. Until now the awarding portal
     * knows the award as {@code myaward1.allocator} on {@code cluster1} and we
     * have nothing to pair it with; approving attaches it to a project of ours -
     * newly created, or one that already exists - and that project's identifier
     * is what closes the loop.
     *
     * <p>A project holds at most one award at a time, so an identifier already
     * attached to a different award is refused. Re-approving with a different
     * identifier <i>moves</i> the award, which is allowed.
     *
     * <p>Nothing is pushed back to the awarding portal, and nothing needs to be.
     * It is already re-sending {@code create_award} every cycle, so the next one
     * gets a {@code ProjectMapping} instead of a pending error - and that
     * mapping is how it learns our identifier. Approval needs no notification
     * path of its own, which is the most useful consequence of the retry
     * contract.
     */
    private JsonNode approve(String offering, String projectId, JsonNode body) {
        Award award = store.award(offering, projectId)
                .orElseThrow(() -> new HttpError(404, "no such award on that offering"));

        String project = text(body, "project");
        String me = app.myPortal();

        // The operator supplies only the project's own name; we qualify it with
        // our portal. Catching a dotted value explicitly rather than letting the
        // parse fail gives the operator the actual answer - "just the project
        // part" - which is the mistake worth anticipating when the full
        // identifier is what appears everywhere else in the API.
        if (project.contains(".")) {
            throw new HttpError(400, "send only the project name, not the full identifier: '"
                    + project.split("\\.")[0] + "' rather than '" + project
                    + "' - the '." + me + "' is added for you");
        }

        ProjectIdentifier local;

        try {
            local = ProjectIdentifier.parse(project + "." + me);
        } catch (IllegalArgumentException e) {
            throw new HttpError(400, "'" + project + "' is not a usable project name: "
                    + e.getMessage()
                    + ". Use 1-64 characters of A-Za-z0-9_- not starting with '-'.");
        }

        // One local project per award. The comparison is on the *whole* key -
        // offering and identifier - because `myaward1.allocator` on cluster1 and
        // the same on cluster2 are two different awards and must not end up
        // sharing one project.
        Optional<Award> clash = store.awardForLocalProjectNow(local.toString());

        if (clash.isPresent() && !clash.get().key().equals(award.key())) {
            throw new HttpError(409, "'" + local + "' is already the local project for "
                    + clash.get().projectId() + " on " + clash.get().offering());
        }

        // Attaching records the date as well as the identifier, because billing
        // is per-day: this award is billed the project's usage from today
        // onwards, and takes the whole of today from whichever award held it
        // before.
        store.attach(award, local.toString(), LocalDate.now());
        award.setReason(body.path("reason").asText(""));
        store.save(award);

        LOG.info("approved " + projectId + " on " + offering + " as " + local);

        ObjectNode answer = Json.object();
        answer.put("offering", offering);
        answer.put("project_id", projectId);
        answer.put("local_project_id", local.toString());
        answer.put("state", award.state());

        ArrayNode attachments = answer.putArray("attachments");
        award.attachments().forEach(attachment -> attachments.add(attachment.json()));

        return answer;
    }

    /**
     * {@code POST /awards/{offering}/{project_id}/reject} - refuse an award,
     * terminally.
     *
     * <p>The reason given here is what the awarding portal receives inside
     * {@code ManagedProjectRejectedError}, so write it for whoever reads it
     * there.
     */
    private JsonNode reject(String offering, String projectId, JsonNode body) {
        Award award = store.award(offering, projectId)
                .orElseThrow(() -> new HttpError(404, "no such award on that offering"));

        String reason = body.path("reason").asText("");

        award.setState(Award.REJECTED);
        award.setReason(reason.isBlank() ? "refused by a site administrator" : reason);
        store.save(award);

        LOG.info("rejected " + projectId + " on " + offering + ": " + award.reason());

        ObjectNode answer = Json.object();
        answer.put("offering", offering);
        answer.put("project_id", projectId);
        answer.put("state", award.state());

        return answer;
    }

    /**
     * {@code PUT /projects/{local_project_id}/usage} - push usage figures in, so
     * {@code get_usage_report} can answer them from cache.
     *
     * <p><b>Note this endpoint is keyed on our own project identifier</b>, not
     * the awarding portal's, and that is deliberate. Everything under
     * {@code /awards} speaks the awarding portal's language because that is the
     * language OpenPortal asks questions in. This one speaks ours, because your
     * accounting produces figures for {@code myproject1.site} and has never
     * heard of {@code myaward1.allocator}.
     *
     * <p>This is the half of the integration that is genuinely yours: your
     * accounting is the source of truth, your parsers produce the numbers, and
     * this endpoint is how they reach the portal.
     *
     * <p><b>The figures are in your own unit</b> - {@link SitePortal#SITE_UNIT},
     * node hours here - and never in the unit an award was allocated in. Push
     * what your accounting produced and let the report builder convert it per
     * award: which award a day belongs to is derived when the report is built,
     * and so is the unit it has to be expressed in, and neither is knowable at
     * the moment a figure arrives.
     *
     * <p>Either shape is accepted, because a real operator has both:
     * {@code hours}, a {@code {date: {email: hours}}} mapping, for a parser that
     * produces numbers; or {@code report}, a complete
     * {@code ProjectUsageReport}, for one that already produces OpenPortal
     * types.
     */
    private JsonNode pushUsage(String localProjectId, JsonNode body) {
        // Deliberately *not* "is an award attached right now". A project whose
        // award has just been removed still needs its last days pushed in - the
        // removed award owns them and the allocator has not necessarily
        // collected them yet.
        if (store.awardsForLocalProject(localProjectId).isEmpty()) {
            throw new HttpError(404, "no award has ever been attached to '" + localProjectId
                    + "' - an award only gets a local project identifier when it is approved");
        }

        Map<LocalDate, Map<String, Double>> hours;

        if (body.hasNonNull("report")) {
            hours = flatten(body.get("report"));
        } else if (body.hasNonNull("hours")) {
            hours = new TreeMap<>();

            body.get("hours").fields().forEachRemaining(day -> {
                Map<String, Double> perUser = new LinkedHashMap<>();
                day.getValue().fields().forEachRemaining(entry ->
                        perUser.put(entry.getKey(), entry.getValue().asDouble()));

                hours.put(LocalDate.parse(day.getKey()), perUser);
            });
        } else {
            throw new HttpError(400, "send either `hours` or `report`");
        }

        store.save(store.project(localProjectId).setUsage(hours));

        // Only to report back where today's figures will land; may be absent if
        // the project is currently unattached, which is not an error.
        Optional<Award> award = store.awardForLocalProjectNow(localProjectId);

        // Which award each day is billed to is deliberately *not* decided here.
        // It depends on the attachment history and can still change - attaching
        // an award this afternoon takes the whole of today - so it is worked out
        // when a report is built, not when figures are recorded.
        ObjectNode answer = Json.object();
        answer.put("local_project_id", localProjectId);
        answer.put("days", hours.size());
        answer.put("billing_to", award.map(Award::projectId).orElse(null));

        return answer;
    }

    /**
     * A pushed {@code ProjectUsageReport} flattened into our own storage shape.
     *
     * <p>Parsing it validates the shape before anything is stored - a malformed
     * report is rejected here rather than failing later, inside a job we have
     * thirty seconds to answer.
     *
     * <p>Walking a report means going date by date: the dates, then one day's
     * report, then the portal-user-to-local-name pairs whose usage that day
     * holds. At the portal layer the local name is the member's email. Only the
     * email is kept, so a report built against either namespace flattens the
     * same way.
     */
    private static Map<LocalDate, Map<String, Double>> flatten(JsonNode raw) {
        ProjectUsageReport report;

        try {
            report = ProjectUsageReport.fromJson(raw);
        } catch (RuntimeException e) {
            throw new HttpError(400, "not a ProjectUsageReport: " + e.getMessage());
        }

        Map<LocalDate, Map<String, Double>> hours = new TreeMap<>();

        for (LocalDate date : report.dates()) {
            DailyProjectUsageReport day = report.getReport(date);
            Map<String, Double> perUser = new LinkedHashMap<>();

            for (Map.Entry<UserIdentifier, String> mapping : report.userMapping().entrySet()) {
                long seconds = day.usage(mapping.getValue()).seconds();

                if (seconds != 0) {
                    perUser.put(mapping.getValue(), seconds / 3600.0);
                }
            }

            if (!perUser.isEmpty()) {
                hours.put(date, perUser);
            }
        }

        return hours;
    }

    /**
     * {@code POST /projects/{local_project_id}/usage/finalise} - declare one
     * month's accounting final, or take that declaration back.
     *
     * <p>This is the endpoint that stops the allocator asking. It sets
     * {@code is_complete} on the days of that month in every
     * {@code get_usage_report} answer, and {@code is_complete} is the
     * allocator's signal that a month is settled and need not be requested
     * again.
     *
     * <p>A deliberately <i>manual</i> decision, and here rather than inside
     * {@link SitePortal} for that reason. Completeness is a claim about the
     * future - "these figures will not change" - and nothing in the code can
     * know it: a scheduler outage, a late job record or a billing correction can
     * all move a figure after the month has ended.
     *
     * <p><b>The current month cannot be finalised.</b> It is still filling: more
     * usage will land in it before it ends, so a claim that its figures are
     * final is one this portal knows to be false.
     *
     * <p>Clearing the flag is always allowed, and does <b>not</b> make the
     * awarding portal ask again. It stopped asking when it recorded the month as
     * settled, and nothing on this side reaches into its records: someone has to
     * tell it, and it un-finalises the month at its end, which is what triggers
     * the refetch. Clearing it here is still right - it stops this portal
     * claiming a month is settled while corrections are still landing - it is
     * simply not what restarts the conversation.
     */
    private JsonNode finaliseUsage(String localProjectId, JsonNode body) {
        if (store.awardsForLocalProject(localProjectId).isEmpty()) {
            throw new HttpError(404, "no award has ever been attached to '"
                    + localProjectId + "'");
        }

        String month = text(body, "month");

        if (!month.matches("^\\d{4}-\\d{2}$")) {
            throw new HttpError(400, "month is 'YYYY-MM', not '" + month + "'");
        }

        boolean isFinal = !body.has("final") || body.get("final").asBoolean(true);
        String thisMonth = SitePortal.monthKey(LocalDate.now());

        if (isFinal && month.compareTo(thisMonth) >= 0) {
            throw new HttpError(400, "cannot declare " + month + " final - it is "
                    + (month.equals(thisMonth) ? "the current month and still filling"
                            : "in the future")
                    + ". Finalise it once it has ended, e.g. '" + previousMonth() + "'.");
        }

        LocalProject project = store.project(localProjectId).setFinal(month, isFinal);
        store.save(project);

        LOG.info((isFinal ? "declared " : "reopened ") + month + " for " + localProjectId);

        ObjectNode answer = Json.object();
        answer.put("local_project_id", localProjectId);
        answer.put("month", month);
        answer.put("final", isFinal);

        ArrayNode settled = answer.putArray("final_months");
        project.finalMonths().forEach(settled::add);

        return answer;
    }

    private static String previousMonth() {
        return SitePortal.monthKey(LocalDate.now().withDayOfMonth(1).minusDays(1));
    }

    // ----------------------------------------------------------------------
    // HTTP plumbing
    // ----------------------------------------------------------------------

    /** A status code and a message, which becomes a {@code {"detail": …}} body. */
    static final class HttpError extends RuntimeException {

        private static final long serialVersionUID = 1L;

        private final int status;

        HttpError(int status, String message) {
            super(message);
            this.status = status;
        }

        int status() {
            return status;
        }
    }

    private static ObjectNode detail(String message) {
        ObjectNode node = Json.object();
        node.put("detail", message);

        return node;
    }

    private static String text(JsonNode body, String field) {
        if (!body.hasNonNull(field)) {
            throw new HttpError(400, "'" + field + "' is required");
        }

        return body.get(field).asText();
    }

    private static List<String> segments(String path) {
        List<String> parts = new ArrayList<>();

        for (String segment : path.split("/")) {
            if (!segment.isEmpty()) {
                parts.add(URLDecoder.decode(segment, StandardCharsets.UTF_8));
            }
        }

        return parts;
    }

    private static Map<String, String> query(HttpExchange exchange) {
        Map<String, String> parameters = new LinkedHashMap<>();
        String raw = exchange.getRequestURI().getRawQuery();

        if (raw == null) {
            return parameters;
        }

        for (String pair : raw.split("&")) {
            int equals = pair.indexOf('=');

            if (equals > 0) {
                parameters.put(
                        URLDecoder.decode(pair.substring(0, equals), StandardCharsets.UTF_8),
                        URLDecoder.decode(pair.substring(equals + 1), StandardCharsets.UTF_8));
            }
        }

        return parameters;
    }

    private static JsonNode body(HttpExchange exchange) throws IOException {
        byte[] raw = exchange.getRequestBody().readAllBytes();

        if (raw.length == 0) {
            return Json.object();
        }

        try {
            return Json.parse(new String(raw, StandardCharsets.UTF_8));
        } catch (RuntimeException e) {
            throw new HttpError(400, "the body is not JSON: " + e.getMessage());
        }
    }

    private static void respond(HttpExchange exchange, int status, JsonNode answer)
            throws IOException {
        byte[] raw = Json.write(answer).getBytes(StandardCharsets.UTF_8);

        exchange.getResponseHeaders().set("Content-Type", "application/json");
        exchange.sendResponseHeaders(status, raw.length);

        try (var out = exchange.getResponseBody()) {
            out.write(raw);
        }
    }
}
