// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal.example;

import java.io.IOException;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.logging.Level;
import java.util.logging.Logger;
import org.openportal.BridgeClient;
import org.openportal.BridgeConfig;
import org.openportal.Destination;
import org.openportal.Job;

/**
 * The site portal, wired up: a bridge connection, an HTTP server, and a sweep.
 *
 * <p>Run it with the bridge's invite file and a port:
 *
 * <pre>
 * java -jar target/site-portal-0.92.0.jar site_bridge.toml 18780 ./portal-state
 * </pre>
 *
 * <p>Three things happen at startup, in this order, and the order matters:
 *
 * <ol>
 *   <li><b>Connect and learn our own name.</b> Everything we advertise is in our
 *       own namespace, so nothing can be registered before we know what it is.
 *       Held rather than re-fetched, because it cannot change while we are
 *       running and approving an award should not depend on the bridge being
 *       reachable at that moment.
 *   <li><b>Register what we offer.</b> Until an offering is registered,
 *       requests for it have nowhere to land - they are <i>held</i> rather than
 *       refused, and delivered once it exists. A fresh portal offers nothing and
 *       starts up perfectly happily; an operator adds a resource and it is
 *       registered from then on.
 *   <li><b>Start the HTTP server, then the sweep.</b> The signal endpoint has to
 *       be answering before anything can be delivered to it.
 * </ol>
 */
public final class App {

    private static final Logger LOG = Logger.getLogger(App.class.getName());

    /** How often the sweep looks for jobs a missed signal left behind. */
    private static final long SWEEP_SECONDS = 30;

    private final BridgeClient bridge;
    private final SitePortal portal;
    private final List<String> awardingPortals;
    private final String myPortal;
    private final OperatorApi api;

    /**
     * The job ids we have already taken on.
     *
     * <p>The same id can be signalled more than once - a retry racing a
     * successful handling - and the work must not happen twice. Bounded by
     * nothing here, which a long-running portal would want to fix; a real one
     * would key de-duplication off the same store the answers go to.
     */
    private final Set<UUID> seen = Collections.synchronizedSet(new HashSet<>());

    private final ExecutorService workers = Executors.newFixedThreadPool(4);
    private final ScheduledExecutorService sweeper = Executors.newSingleThreadScheduledExecutor();

    private volatile Set<String> registered = Set.of();

    App(BridgeClient bridge, SitePortal portal, List<String> awardingPortals) {
        this.bridge = bridge;
        this.portal = portal;
        this.awardingPortals = List.copyOf(awardingPortals);

        // Learned once, at construction, for the reason in the class docs.
        this.myPortal = bridge.getPortal();
        this.api = new OperatorApi(this);
    }

    public BridgeClient bridge() {
        return bridge;
    }

    public SitePortal portal() {
        return portal;
    }

    /** Our own portal's agent name. */
    public String myPortal() {
        return myPortal;
    }

    /**
     * The portals allowed to make awards here.
     *
     * <p>A real portal reads this from its own configuration, and would let an
     * operator manage it the same way the resources are managed. It is an
     * argument here so that the example has one fewer moving part.
     */
    public List<String> awardingPortals() {
        return awardingPortals;
    }

    /**
     * The wire names of one resource: {@code <offering>.<us>.<them>}, one per
     * awarding portal.
     *
     * <p>The middle element must be our own agent name - an offering in somebody
     * else's namespace is not something this portal can advertise, and the
     * portal agent rejects it. One registration per (resource, awarding portal)
     * pair, because each is a separate virtual agent that that portal may
     * address.
     */
    public List<String> destinationsFor(String offering) {
        List<String> destinations = new ArrayList<>();

        for (String them : awardingPortals) {
            destinations.add(Destination.parse(offering + "." + myPortal + "." + them).toString());
        }

        return destinations;
    }

    /**
     * Tell OpenPortal the complete set of resources we advertise, and return
     * what it accepted.
     *
     * <p>{@code sync_offerings} is a <b>replace</b>, not a merge: anything
     * absent is withdrawn, and an empty set withdraws everything. That is what
     * makes this one call enough for adding and removing alike - there is no
     * separate "unregister".
     */
    public Set<String> publishOfferings() {
        List<Destination> offerings = new ArrayList<>();

        for (String offering : portal.offeringNames()) {
            for (String destination : destinationsFor(offering)) {
                offerings.add(Destination.parse(destination));
            }
        }

        Set<String> active = new java.util.LinkedHashSet<>();
        bridge.syncOfferings(offerings).forEach(destination -> active.add(destination.toString()));

        LOG.info("registered offerings: " + active);
        registered = Set.copyOf(active);

        return registered;
    }

    /** What OpenPortal held as our offerings the last time we asked. */
    public Set<String> registeredDestinations() {
        return registered;
    }

    /** Take on a job id, or report that we already have. */
    boolean claim(UUID id) {
        return seen.add(id);
    }

    /** Give an id back, when the fetch it was claimed for did not happen. */
    void release(UUID id) {
        seen.remove(id);
    }

    /** Run something off the request thread, so the signal endpoint returns at once. */
    void background(Runnable work) {
        workers.execute(work);
    }

    /**
     * Poll for jobs that a missed signal left behind.
     *
     * <p>The signal is the primary path and this is the safety net. It exists
     * because a signal can be lost - a restart at the wrong moment, a network
     * blip - and without it that job would sit on the board until it expired.
     */
    private void sweep() {
        try {
            for (Job job : bridge.fetchJobs()) {
                if (!claim(job.id())) {
                    continue;
                }

                LOG.info("sweep picked up job " + job.id() + " that no signal delivered");
                portal.handle(bridge, job);
            }
        } catch (RuntimeException e) {
            // The sweep must never die.
            LOG.log(Level.WARNING, "sweep failed; will retry", e);
        }
    }

    int start(int port) throws IOException {
        publishOfferings();

        int listening = api.start(port);

        sweeper.scheduleWithFixedDelay(
                this::sweep, SWEEP_SECONDS, SWEEP_SECONDS, TimeUnit.SECONDS);

        return listening;
    }

    void stop() {
        sweeper.shutdownNow();
        workers.shutdownNow();
        api.stop();
    }

    public static void main(String[] args) throws IOException {
        if (args.length < 2) {
            System.err.println("""
                    usage: site-portal <bridge invite file> <port> [state directory]

                      <bridge invite file>  the site bridge's config, e.g.
                                            ../../../python/examples/site_portal/data/python/site_bridge.toml
                      <port>                the port to serve on - the same one the bridge's
                                            signal_url names, or nothing will be delivered
                      [state directory]     where to keep the awards (default ./portal-state)

                    The awarding portals are read from PORTAL_AWARDING_PORTALS
                    (comma-separated, default "allocator").""");

            System.exit(2);
        }

        Path invite = Path.of(args[0]);
        int port = Integer.parseInt(args[1]);
        Path state = Path.of(args.length > 2 ? args[2] : "./portal-state");

        List<String> awarding = new ArrayList<>();

        for (String them : System.getenv()
                .getOrDefault("PORTAL_AWARDING_PORTALS", "allocator")
                .split(",")) {
            if (!them.isBlank()) {
                awarding.add(them.trim());
            }
        }

        BridgeClient bridge = new BridgeClient(BridgeConfig.load(invite));
        App app = new App(bridge, new SitePortal(new Store(state)), awarding);

        Runtime.getRuntime().addShutdownHook(new Thread(app::stop));

        int listening = app.start(port);

        System.out.println("site portal '" + app.myPortal() + "' listening on http://127.0.0.1:"
                + listening);
        System.out.println("  bridge:    " + bridge.url());
        System.out.println("  state:     " + state.toAbsolutePath());
        System.out.println("  awarding:  " + String.join(", ", app.awardingPortals()));
        System.out.println("  offerings: "
                + (app.portal().offeringNames().isEmpty()
                        ? "none yet - POST /offerings to add one"
                        : String.join(", ", app.portal().offeringNames())));
    }
}
