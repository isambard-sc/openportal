// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import java.nio.file.Path;
import java.util.List;

/**
 * Answer whatever is on the bridge board, and say what happened.
 *
 * <p>The other half of {@link LiveBridgeCheck}: that one proves the calls are
 * accepted, this one proves a job can be taken off the board, answered, and that
 * the answer reaches the portal that asked. Every award instruction is answered
 * with {@link ManagedProjectPendingError}, which is what a portal that queues
 * awards for human approval says - so an awarding portal on the other side sees
 * a pending award rather than a broken one.
 */
public final class LiveJobCheck {

    public static void main(String[] args) throws Exception {
        BridgeClient bridge = BridgeClient.load(Path.of(args[0]));
        List<Job> jobs = bridge.fetchJobs();

        System.out.println("fetch_jobs: " + jobs.size() + " on the board");

        for (Job job : jobs) {
            Instruction instruction = job.instruction();

            System.out.println("  job         " + job.id());
            System.out.println("    command   " + job.command());
            System.out.println("    verb      " + instruction.command());
            System.out.println("    arguments " + instruction.arguments());
            System.out.println("    from      " + job.forwardedFor().map(Object::toString).orElse("(local)"));
            System.out.println("    offering  " + job.forwardedFor().map(Destination::last).orElse(job.destination().last()));

            Job answered =
                    job.errored(
                            new ManagedProjectPendingError("awaiting approval by a site administrator"));

            System.out.println("    answering " + answered.state().wire()
                    + " kind=" + answered.errorKind()
                    + " version " + job.version() + " -> " + answered.version());

            bridge.sendResult(answered);
        }

        System.out.println(jobs.isEmpty() ? "nothing to answer" : "answered " + jobs.size());
    }
}
