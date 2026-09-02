// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.nio.file.Path;
import java.util.List;

/**
 * Drive a real bridge, and say so.
 *
 * <p>Not a unit test - it needs a running {@code op-bridge} - and that is the
 * point: the signature is either byte-identical to what the bridge computes or
 * it is not, and only a bridge can settle it. The unit tests beside this cover
 * the pieces that are pure functions.
 *
 * <pre>
 * cd python/examples/site_portal &amp;&amp; python example.py start
 * java -cp … org.openportal.LiveBridgeCheck data/python/site_bridge.toml
 * </pre>
 */
public final class LiveBridgeCheck {

    public static void main(String[] args) throws Exception {
        if (args.length < 1) {
            System.err.println("usage: LiveBridgeCheck <bridge-invite.toml>");
            System.exit(2);
        }

        BridgeClient bridge = BridgeClient.load(Path.of(args[0]));
        System.out.println("bridge:          " + bridge.url());

        String me = bridge.getPortal();
        System.out.println("get_portal:      " + me);

        System.out.println("health:          " + bridge.health().status());

        List<Destination> before = bridge.getOfferings();
        System.out.println("get_offerings:   " + before);

        List<Destination> offerings =
                List.of(
                        Destination.parse("cluster1." + me + ".allocator"),
                        Destination.parse("cluster2." + me + ".allocator"));
        System.out.println("sync_offerings:  " + bridge.syncOfferings(offerings));

        System.out.println("fetch_jobs:      " + bridge.fetchJobs().size() + " outstanding");

        // Submit something, poll it, and read the answer back. The command is
        // deliberately one the bridge will refuse, because that exercises the
        // more interesting path: a failure travels back as text and has to
        // decode to a typed error on this side.
        Job job = bridge.run(me + ".site_bridge get_offerings");
        System.out.println("run:             " + job.id() + " " + job.state().wire());

        Job finished = bridge.waitFor(job, java.time.Duration.ofSeconds(20));
        System.out.println("...finished as:  " + finished.state().wire()
                + " (" + finished.error().map(e -> e.getClass().getSimpleName()).orElse("no error") + ")");
        System.out.println("...saying:       "
                + finished.error().map(Throwable::getMessage).orElse(finished.resultText().orElse("")));

        // Put the offerings back as they were, so this leaves nothing behind.
        bridge.syncOfferings(before);
        System.out.println("restored:        " + bridge.getOfferings());
        System.out.println("\nOK - every call was accepted by the bridge");
    }
}
