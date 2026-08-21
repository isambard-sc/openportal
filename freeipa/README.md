<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# FreeIPA agent

This requires extra configuration to set the details used to connect
to the FreeIPA server.

To test, the demo server provided by FreeIPA is very useful.
This is at [ipa.demo1.freeipa.org](https://ipa.demo1.freeipa.org/),
and you can use the username `admin` and password `Secret123`.

First, turn on simple encryption for the FreeIPA password

```bash
op-freeipa encryption --simple
```

You set the server details using

```bash
op-freeipa extra -k freeipa-server -v https://ipa.demo1.freeipa.org
op-freeipa extra -k freeipa-user -v admin
op-freeipa secret -k freeipa-password -v Secret123
```

You can also add the set of system groups that should always be used
when adding users to FreeIPA via this agent. This should be a
comma-separated list of group names.

```bash
op-freeipa extra -k system-groups -v group1,group2
```

## Multi-master topologies

`freeipa-server` may list several servers, comma-separated. Reads are spread
over all of them; every write goes to one, named by `freeipa-write-server` and
defaulting to the first entry.

This is not load balancing that happens to be configurable - it is required.
FreeIPA's replication cannot reconcile two independent `ADD`s of the same DN,
and the conflict entry it leaves behind is invisible to `ipa user-find` and
cannot be deleted through the IPA framework at all. So each entry must name an
individual server: a VIP or a round-robin alias is several masters behind one
name, and pinning writes to it pins nothing.

Writes go to one server at a time, but which server can change - if the write
server is confirmed down for longer than `freeipa-replication-window`, one
replacement is elected in configuration order, reverting once the original has
been up again for a full window.

Writes have connections of their own, `freeipa-concurrent-writes` of them
(default 2), opened on whichever server currently holds the role. So write
concurrency follows the role across a failover, and raising it does not also
multiply the connections that reads share - each server normally only needs
listing once. Listing a server more than once still gives reads more
connections to it.

```bash
op-freeipa extra -k freeipa-server -v https://ipa1.example.com,https://ipa2.example.com,https://ipa3.example.com
op-freeipa extra -k freeipa-write-server -v https://ipa1.example.com
op-freeipa extra -k freeipa-concurrent-writes -v 4
```

To find conflict entries that already exist - the agent reports the risk of
creating one under the `REPLICATION-RISK` marker, but nothing self-heals the
ones already in the directory:

```bash
kinit admin
scripts/check-replication-conflicts.sh ldaps://ipa1.example.com ldaps://ipa2.example.com
```
