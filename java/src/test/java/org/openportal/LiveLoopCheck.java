// SPDX-FileCopyrightText: © 2026 Christopher Woods <Christopher.Woods@bristol.ac.uk>
// SPDX-License-Identifier: MIT

package org.openportal;

import com.sun.net.httpserver.HttpServer;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executors;

/**
 * The whole cycle, in Java: be signalled, fetch, answer.
 *
 * <p>The last thing a client has to prove. {@link LiveBridgeCheck} shows the
 * calls are accepted and {@link LiveJobCheck} shows a job can be answered; this
 * one stands up the {@code signal_url} the bridge was initialised with, so a job
 * arriving from another portal is delivered, answered, and the answer travels
 * back to the portal that asked.
 *
 * <p>The HTTP here is the JDK's own {@link HttpServer} - enough to prove the
 * point without choosing a framework for anybody. Three things it gets right,
 * and they are the same three the Python example's {@code /signal/job} gets
 * right: it returns at once and does the work afterwards, it treats the job id as
 * the credential and answers an unknown one with 403, and it de-duplicates,
 * because the same id can arrive twice.
 */
public final class LiveLoopCheck {

    private static final Map<String, Boolean> SEEN = new ConcurrentHashMap<>();

    public static void main(String[] args) throws Exception {
        BridgeClient bridge = BridgeClient.load(Path.of(args[0]));
        int port = Integer.parseInt(args[1]);
        int seconds = args.length > 2 ? Integer.parseInt(args[2]) : 30;

        HttpServer server = HttpServer.create(new InetSocketAddress("127.0.0.1", port), 0);
        server.setExecutor(Executors.newFixedThreadPool(4));

        server.createContext(
                "/signal/job",
                exchange -> {
                    String id = query(exchange.getRequestURI().getQuery(), "job_id");
                    System.out.println("signalled: " + id);

                    if (id == null || SEEN.putIfAbsent(id, true) != null) {
                        respond(exchange, 200, "{}");
                        return;
                    }

                    // 200 first, work afterwards: the bridge retries a failed
                    // signal five times and then errors the job, so this must not
                    // wait for the answer to be computed.
                    respond(exchange, 200, "{}");

                    try {
                        Job job = bridge.fetchJob(UUID.fromString(id));
                        System.out.println("  fetched: " + job.instruction().command()
                                + " through " + job.forwardedFor().map(Destination::last).orElse("(local)"));

                        Job answered =
                                job.errored(
                                        new ManagedProjectPendingError(
                                                "awaiting approval by a site administrator"));
                        bridge.sendResult(answered);
                        System.out.println("  answered: " + answered.errorKind());
                    } catch (RuntimeException e) {
                        System.out.println("  failed: " + e);
                    }
                });

        server.createContext(
                "/signal/notification",
                exchange -> {
                    System.out.println("notification: " + exchange.getRequestURI().getQuery());
                    respond(exchange, 200, "{}");
                });

        server.start();
        System.out.println("listening on http://127.0.0.1:" + port + "/signal/job for " + seconds + "s");

        Thread.sleep(seconds * 1000L);
        server.stop(0);
        System.out.println("stopped");
    }

    private static String query(String query, String key) {
        if (query == null) {
            return null;
        }

        for (String pair : query.split("&")) {
            int equals = pair.indexOf('=');

            if (equals > 0 && pair.substring(0, equals).equals(key)) {
                return pair.substring(equals + 1);
            }
        }

        return null;
    }

    private static void respond(com.sun.net.httpserver.HttpExchange exchange, int status, String body)
            throws IOException {
        byte[] payload = body.getBytes(StandardCharsets.UTF_8);
        exchange.getResponseHeaders().set("Content-Type", "application/json");
        exchange.sendResponseHeaders(status, payload.length);

        try (OutputStream out = exchange.getResponseBody()) {
            out.write(payload);
        }
    }
}
