# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

## [0.9.5] - 2025-02-20
### Added
- Added an environment variable to turn on checking of the user
  class in FreeIPA. This is the double-check that isn't really needed
  and gets in the way now. The default is to not check the user
  class is "openportal". Setting the environment variable
  `OPENPORTAL_REQUIRE_MANAGED_CLASS` to `true` will turn on the check.

### Fixed
- Made the logic for modifying users in FreeIPA more robust - now always
  re-fetch if the user is in the openportal group so that this info
  is always up to date.
- Cleaned up the logic for removal - a user will be removed even if
  they aren't in any of the resource instance groups. This removed an
  edge case where they were not in a resource instance group, but were
  still active, but openportal would not remove them.

## [0.9.4] - 2025-02-20
### Fixed
- Made sure that RUST_LOG_FORMAT is configurable from the helm chart.

## [0.9.3] - 2025-02-20
### Added
- Added configurable logging - output now respects the value of the
  `RUST_LOG` environment variable, using the standard `env_logger` crate.
- Added json logging, which is controlled by the `RUST_LOG_FORMAT` environment
  variable. If this is set to `json`, then logs will be output in JSON format.
- Fixed a communications flood caused by a connection not detecting if
  multiple watchdog messages are already in flight. Now only a single
  watchdog message is pending send, using the same mechanism as the
  keepalive messages.

## [0.9.2] - 2025-02-19
### Fixed
- Fixed incorrect handling of the `cluster` field in slurm that meant
  that race conditions prevented users and accounts from being properly
  added to multiple clusters within the same slurmd instance.

## [0.9.1] - 2025-02-18
### Added
- Added a command to force a disconnect of an open connection. Changed
  the keepalive logic so that, if a keepalive message can't be sent,
  then the connection is automatically disconnected and remade. This
  should prevent hangs caused by one half of a connection being down.
- Added a "last activity" tracker to the connections, and a periodic
  watchdog that checks for connections that have been inactive for
  more than 5 minutes (much greater than the keepalive period).
  This will automatically disconnect the connection, and log a warning,
  with the connection automatically remade. This should prevent
  connections getting stuck in a stuck half-open state.
- Updated to support the latest version of rust, plus to use the latest
  version of all dependencies. This includes upgrading to the new
  secrecy 0.10 from 0.8, which required internal code changes. This
  doesn't impact anything external.

## [0.9.0] - 2025-02-10
### Added
- Added instructions to ask for the home and project directories for a
  user and project.
- Changed the order of creating a user account, so that now `op-freeipa`
  will ask `op-filesystem` for the expected home account details before
  actually creating the account. This way, the home directory can be
  part of the account creation process, preventing FreeIPA from triggering
  the creation of home directories in the wrong place.
- Added FreeIPA groups that record which OpenPortal instances a user is
  a member of. This lets OpenPortal know if a user is a member of multiple
  instances, thus preventing removing a user from one instance from
  removing them from all instances. This also adds some additional layers
  of protection against accidental removal of users from instances.
- Added mutex locking around adding / removing individual users in
  `op-freeipa` and `op-slurm`, and around each directory creation
  operation in `op-filesystem`. This removes the possibility of many
  race conditions, and that we aren't going to accidentally try to
  add and remove a single user at the same time. New processes try to
  get the lock for 10 seconds, and if they can't, they will return an
  error.

## [0.8.3] - 2025-02-06
### Fixed
- Improved logging to reduce chattiness and improve clarity
- Reduced timeout values so that missing agents won't cause the system
  to get too stuck in loops

## [0.8.2] - 2025-02-06
### Fixed
- Extra protections to ensure that agents are connected to the cluster
  before it attempts anything, and to return valid results if existing
  protected users exist

## [0.8.1] - 2025-02-06
### Fixed
- Stopped the freeipa agent from removing groups! This can lead to GID
  information being lost, and is not what we want. Instead, we now
  remove the user from the group, and leave the group alone. Now, if the
  group with the same name is recreated, it will recover its previous
  GID.

## [0.8.0] - 2025-02-05
### Added
- Added a "is_protected_user" instruction, to allow querying for user accounts
  that should not be managed by OpenPortal. This is useful for accounts that
  exist and are managed by other systems, but which need to be seen by
  portals interfacing via OpenPortal

## [0.7.0] - 2025-02-04
### Added
- Added in convenience functions to the Python API to make it easier
  to query dates.

## [0.6.2] - 2025-02-04
### Fixed
- General bugfixes in how the slurm accounting evaluated job consumption data.
- General bugfixes related to how agents handle mulitple slurm clusters.

## [0.6.1] - 2025-02-03
### Added
- Added support for legacy BriCS accounts and projects

## [0.6.0] - 2025-01-27
### Added
- Added commands to get and set usage limits. These are recorded, but
  not yet translated into slurm (that will be for a future release - currently
  they are just used to link with Waldur).
- Added lots of convenience functions and converters for date ranges,
  to make requesting of older reports easer.
- Added lots of converters for usage quantities, plus converters for
  constructors. Prettier print output too.

## [0.5.0] - 2025-01-23
### Added
- Added full accounting support. Can now get accounting data from slurm
  and return this as `UsageReport` and `ProjectUsageReport` objects
  that are also accessible from Python.
- Cleaned up the logging so the output is cleaner and easier to follow
- Made the FreeIPA interface even more robust, handling even more errors
  and edge cases.

## [0.4.0] - 2025-01-03
### Added
- Added per-message encryption keys, using a per-connection pair of
  random salts and randomly generated additional infos per message.
  This is a breaking change in the communication format, so agents
  older that this release will not be able to communicate with
  newer agents.
- Added the ability to construct most of the python-exposed objects
  in Python by mapping the parse functions to Python constructors.
  This will make it easier to save objects to strings, and then
  reconstruct as needed.
- Added the ability to ignore invalid SSL certificates when connecting
  to a FreeIPA server, if the environment variable
  `OPENPORTAL_ALLOW_INVALID_SSL_CERTS` is equal to `true`. The default
  is `false`, so that invalid certificates are not allowed.
  This should only be used in development or debugging, as use
  in production is a security risk.
- Added a check so we can't query projects from the wrong portal.

## [0.3.0] - 2024-12-23
### Added
- Added a `PortalIdentifier` so that we are clean in how we identify
  the three parts; User, Project and Portal
- Added parse pattern for all identifiers - they can now only
  be parsed, and will always be valid if created.
- Added functions to list projects and users, so that we can now
  fully integrate with Waldur. These cache the results from FreeIPA,
  so shouldn't hit the server too hard.
- Added functions to remove users and projects, which are fully
  functional for FreeIPA and stubbed for slurm and filesystem.
  Removed users are disabled in FreeIPA, and are re-enabled
  if they are re-added. This ensures that their stats plus their
  UIDs etc are preserved. Removing a project will remove all
  of the users in the project.
- Added new Python return types, namely Vector/List versions of all
  of the base types (`String`, `UserIdentifier`, `ProjectIdentifier`, etc),
  plus the new `PortalIdentifier` and `Vec<PortalIdentifier>`.
  This triggered the bumped minor version as the API has changed.

## [0.2.0] - 2024-12-17
### Added
- Added some extra functions to the Python layer to make it easier to
  integrate OpenPortal with, e.g. Waldur. These include `is_config_loaded`
  to check if the config has been loaded, and `get` to get the
  Job that matches the passed ID.
- Added automatic building of Python Linux aarch64 binaries, so that
  the Python module can be used on ARM64 systems.
- Cleaned up the Python API and added in lots of convenience functions.
  Objects are now correctly returned from the `run` function, so that you
  don't need to parse anything. Also added in the ability to default
  wait for a command to run
- Added in extra commands to add and remove projects, list users in a
  project, and list projects in a portal. Some of these are still stubbed.
- Added in `ProjectIdentifier` and `ProjectMapping` to mirror the
  equivalent `User` classes. Also cleaned up the concept of local
  users and groups, so that a `UserMapping` maps a user to a local
  unix username and unix group, while the `ProjectMapping` maps a
  project to a local unix group.

## [0.1.1] - 2024-12-02
### Added
- Added `instance_groups` to the FreeIPA agent, so that is is possible to
  specify additional groups that a user should be added to when they are
  added from a specific instance. This is useful when multiple instances
  share the same freeipa agent, and you want to add them to different groups.

## [0.1.0] - 2024-11-26
### Added
- Added full recovery support, so that agents can restore their boards
  after they restart. Also added a queue, so that messages are queued
  if the agent is down. Plus added a wait when looking for agents, so that
  time is given for an agent to first connect and identify itself. All of
  this makes the system more robust and reliable, as most jobs are now
  tolerant of individual agents going down.
- Added a better handshake so that agents communicate both their comms
  engine details (e.g. paddington version 0.0.25) and their agent
  engine details (e.g. templemeads version 0.0.25). This will future proof
  us if we make any future changes to the protocols. Note that this
  is BREAKING, so agents cannot commnunicate with older versions of
  openportal
- Added an expiry to jobs, default to 1 minute, that means that both
  jobs are now cleaned automatically from boards once expired (by a
  quiet background tokio task), and that putter of jobs can get a signal
  that the job has expired, and thus return an error, if the job gets
  lost in the system. This is a breaking change, as the job expiry
  is a new field. It again significantly improves the robustness of the
  system, both stopping putters getting stuck indefinitely, and also
  preventing memory exhaustion by jobs that are never cleaned up. Have
  set the bridge agent to put jobs with a expiry of 60 minutes, so that
  there is plenty of time for the web portal to fetch the results without
  worrying about them being expired.
- Added a command line support for the slurm agent, so that it can use
  `sacctmgr` to create accounts on slurm in addition to the REST API.
  You choose the command line option by not setting the `slurm-server`
  value in the config file.

### Fixed
- General bug fixes and cleaning of output logging to improve resilience
  and make it easier to debug issues.

## [0.0.25] - 2024-11-20
### Fixed
- Fixed attestation issue for slurm container

## [0.0.24] - 2024-11-20
### Added
- Added control over the lifetime of the slurm JWT token, plus a check
  to automatically refresh the token before it expires.

### Fixed
- Fixed the lack of op-slurm containers and helm charts - these are now
  built automatically by GH Actions

## [0.0.23] - 2024-11-19
### Added
- Added in a slurm agent as an example of an accounting agent. This can
  now create accounting accounts on slurm when a user is added to
  a cluster. The slurm account is created with the mapped username
  and project name via the `add_local_user` command, in a similar
  way to how the filesystem agent works. This uses the slurm REST
  API to create and manage the account, using JWT tokens for
  authentication.

## [0.0.22] - 2024-11-13
### Added
- Finished the "AddLocalUser" command for the filesystem agent. User home
  dirs and project dirs are now created, following admin settings. This
  includes multiple project dirs, plus links between dirs. Multiple checks
  ensure that directories are only created if they don't exist, and that
  they aren't created if the user or group don't exist. Also, checks to
  ensure that they aren't written to anywhere sensitive on the filesystem.

## [0.0.21] - 2024-11-12
### Added
- Moved all command and grammar parsing fully over to the parse pattern.
  You cannot now create any commands that aren't valid. Added lots of
  extra tests of validity, e.g. that commands that impact users must
  come from the portal that manages that user.
- Separated out the bridge so that it communicates via the portal in a
  different zone. Added a "submit" command that is only used by the
  bridge to submit instructions to the portal. Added lots of strict
  validation to ensure the bridge<=>portal connection is verified and
  all comamnds are sane, and pass all of the about parse tests.
- Related to the above, changed commands so that you now don't specify
  the bridge<=>portal connection when submitting commands via python.
  You would now do "portal.provider.platform.instance add_user user.project.portal",
  rather than "bridge.portal.provider.plaform.instance ...". It is a small
  change, but it is easier to understand, and now the bridge is just
  an invisible bridge between the "normal" work and the OpenPortal world.

## [0.0.20] - 2024-11-08
### Added
- Added the concept of zones. Agents can now only send messages along chains
  within the same zone. This increases security, and makes it easier to
  segment the agent peer network into different zones (with some agents
  acting as bridges between multiple zones).

## [0.0.19] - 2024-11-07
### Fixed
- Made the code more robust to freeipa being cleared / having groups removed
  behind our back. Also better way to handle errors.

## [0.0.18] - 2024-11-05
### Fixed
- Specified default TLS provider so that containerised services can run without
  panicing.

## [0.0.17] - 2024-11-01
### Fixed
- Fixed issues with attestations that depended on releases. Need to release
  each agent separately, which this release now does.

## [0.0.16] - 2024-11-01
### Fixed
- Fixed issue with attestation of OCI images

## [0.0.15] - 2024-11-01
### Fixed
- Fixed issues with the helm charts and OCI images (removed `op-platform` as it
  doesn't exist!)

## [0.0.14] - 2024-11-01
### Added
- Changed the names of the cluster instance and platform agents to `cluster` and `clusters`,
  as they don't need to be named after slurm (and would cause confusion with the slurm agent).
- Added OCI images and helm charts for all agents
- Added instructions on how to configure the freeipa agent

## [0.0.12] - 2024-10-28
### Added
- Added support for keepalive messages so that connections are kept open

## [0.0.11] - 2024-10-28
### Added
- Fixed bug in handling of client proxy IP - need to use IP not port ;-)

## [0.0.10] - 2024-10-25
### Added
- Fixed bug in parsing header proxy IP address

## [0.0.9] - 2024-10-25
### Added
- Fixed bug in parsing command line options for bridge
- Added support for getting the client IP address from a proxy header (e.g. `X-Forwarded-For`)
- Cleaned up port handling, so URLs with default ports don't have the ports specified

## [0.0.8] - 2024-10-24
### Added
- Added names for the ports in the helm charts

## [0.0.7] - 2024-10-24
### Added
- Added a healthcheck server to simplify pod healthchecks
- Updated helm charts to use the healthcheck server, plus expose the bridge server port

## [0.0.6] - 2024-10-23
### Added
- Separated out build artefacts so that they can be picked up by the rest of the build

## [0.0.5] - 2024-10-23
### Added
- Fixing generation and attestation of SBOMs for container images (finally!)

## [0.0.4] - 2024-10-23
### Added
- Fixing release issues, and beginning work on the workflow for the Python module

## [0.0.3] - 2024-10-23
### Added
- Fixing the attestations so that SBOMs are correctly generated for container images.

## [0.0.2] - 2024-10-23
### Added
- Fixing the helm charts so that they version numbers are correctly set.

## [0.0.1] - 2024-10-23
### Changed
- Initial release
  This is an initial alpha release of the OpenPortal project. It is not yet feature complete and is not recommended for production use.

[0.9.5]: https://github.com/isambard-sc/openportal/releases/tag/0.9.5
[0.9.4]: https://github.com/isambard-sc/openportal/releases/tag/0.9.4
[0.9.3]: https://github.com/isambard-sc/openportal/releases/tag/0.9.3
[0.9.2]: https://github.com/isambard-sc/openportal/releases/tag/0.9.2
[0.9.1]: https://github.com/isambard-sc/openportal/releases/tag/0.9.1
[0.9.0]: https://github.com/isambard-sc/openportal/releases/tag/0.9.0
[0.8.3]: https://github.com/isambard-sc/openportal/releases/tag/0.8.3
[0.8.2]: https://github.com/isambard-sc/openportal/releases/tag/0.8.2
[0.8.1]: https://github.com/isambard-sc/openportal/releases/tag/0.8.1
[0.8.0]: https://github.com/isambard-sc/openportal/releases/tag/0.8.0
[0.7.0]: https://github.com/isambard-sc/openportal/releases/tag/0.7.0
[0.6.2]: https://github.com/isambard-sc/openportal/releases/tag/0.6.2
[0.6.1]: https://github.com/isambard-sc/openportal/releases/tag/0.6.1
[0.6.0]: https://github.com/isambard-sc/openportal/releases/tag/0.6.0
[0.5.0]: https://github.com/isambard-sc/openportal/releases/tag/0.5.0
[0.4.0]: https://github.com/isambard-sc/openportal/releases/tag/0.4.0
[0.3.0]: https://github.com/isambard-sc/openportal/releases/tag/0.3.0
[0.2.0]: https://github.com/isambard-sc/openportal/releases/tag/0.2.0
[0.1.1]: https://github.com/isambard-sc/openportal/releases/tag/0.1.1
[0.1.0]: https://github.com/isambard-sc/openportal/releases/tag/0.1.0
[0.0.25]: https://github.com/isambard-sc/openportal/releases/tag/0.0.25
[0.0.24]: https://github.com/isambard-sc/openportal/releases/tag/0.0.24
[0.0.23]: https://github.com/isambard-sc/openportal/releases/tag/0.0.23
[0.0.22]: https://github.com/isambard-sc/openportal/releases/tag/0.0.22
[0.0.21]: https://github.com/isambard-sc/openportal/releases/tag/0.0.21
[0.0.20]: https://github.com/isambard-sc/openportal/releases/tag/0.0.20
[0.0.19]: https://github.com/isambard-sc/openportal/releases/tag/0.0.19
[0.0.18]: https://github.com/isambard-sc/openportal/releases/tag/0.0.18
[0.0.17]: https://github.com/isambard-sc/openportal/releases/tag/0.0.17
[0.0.16]: https://github.com/isambard-sc/openportal/releases/tag/0.0.16
[0.0.15]: https://github.com/isambard-sc/openportal/releases/tag/0.0.15
[0.0.14]: https://github.com/isambard-sc/openportal/releases/tag/0.0.14
[0.0.12]: https://github.com/isambard-sc/openportal/releases/tag/0.0.12
[0.0.11]: https://github.com/isambard-sc/openportal/releases/tag/0.0.11
[0.0.10]: https://github.com/isambard-sc/openportal/releases/tag/0.0.10
[0.0.9]: https://github.com/isambard-sc/openportal/releases/tag/0.0.9
[0.0.8]: https://github.com/isambard-sc/openportal/releases/tag/0.0.8
[0.0.7]: https://github.com/isambard-sc/openportal/releases/tag/0.0.7
[0.0.6]: https://github.com/isambard-sc/openportal/releases/tag/0.0.6
[0.0.5]: https://github.com/isambard-sc/openportal/releases/tag/0.0.5
[0.0.4]: https://github.com/isambard-sc/openportal/releases/tag/0.0.4
[0.0.3]: https://github.com/isambard-sc/openportal/releases/tag/0.0.3
[0.0.2]: https://github.com/isambard-sc/openportal/releases/tag/0.0.2
[0.0.1]: https://github.com/isambard-sc/openportal/releases/tag/0.0.1
