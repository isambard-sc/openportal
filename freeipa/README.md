<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# FreeIPA agent

This requires extra configuration to set the details used to connect
to the FreeIPA server.

To test, the demo server provided by FreeIPA is very useful.
This is at `[ipa.demo1.freeipa.org](https://ipa.demo1.freeipa.org/),
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
