<!--
SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
SPDX-License-Identifier: CC0-1.0
-->

# OpenPortal

This is an implementation of the OpenPortal protocol for communication
between a user portal (e.g. Waldur) and digital research infrastructure
(e.g. the Isambard supercomputers provided by BriSC).

The aim of OpenPortal is to separate out the communication of business logic
of deploying, e.g. new user accounts on a supercomputer, from the actual
implementation of that logic, and the web interfaces to drive, e.g.
user and project management via a web portal.

OpenPortal sits in the backend behind user and project management portals,
communicating the business logic (e.g. add user "joe" to project "myproject"
on cluster "phase1") to that backend infrastructure. Then, agents
autonomously respond to that communication to actually do the work of
implementing that logic, e.g. by adding the user to FreeIPA, setting up
a Slurm account, creating home and project directories etc.

The protocol makes it easy to add new agents, and to specialise them to
your own infrastructure.

## Python Wrapper

OpenPortal is written in Rust, but many user portals are written in Python.
This Python module is an interface to OpenPortal from Python. It lets
you submit OpenPortal jobs and receive the results. It provides a thin
wrapper to the OpenPortal "Bridge" agent, which bridges between the
asynchronous real-time push model of OpenPortal to the more synchronous,
RESTful pull model of many Python clients.

## Installation

You should be able to install via pip:

```bash
pip install openportal
```

Binaries for Linux X86-64 and Linux aarch64 are provided. If you need to
compile yourself, then it is easiest to follow the instructions below
to compile using `maturin`.

## Developing the Python wrappers

First, clone the repository, e.g.

```bash
git clone https://github.com/isambard-sc/openportal
```

Then change into the `python` directory, e.g.

```
cd openportal/python
```

To compile these wrappers yourself, you should create a Python
virtual environment into which to develop / install the wrappers.

For example;

```
$ python -m venv .env
$ source .env/bin/activate
```

Next, you need to install `maturin`

```bash
$ pip install maturin
```

Then you can compile and install the wrappers using `maturin`, e.g.

```bash
$ maturin develop
```

## Using the wrappers

You need to have created an OpenPortal invitation / configuration from
your bridge agent, e.g. via

```bash
$ openportal-bridge bridge --config python_config.toml
```

This contains the secrets you need to connect your Python client.
Keep these secrets safe!

Then you can use the Python client to submit jobs, e.g.

```python
import openportal
import time

openportal.load_config("python_config.toml")

print(openportal.health())

job = openportal.run("portal.provider.platform.instance add_user person.project.portal")

job.update()

print(job)

while not job.is_finished:
    time.sleep(0.1)
    job.update()

if job.is_error:
    raise ValueError(f"Error: {job.error}")

print(f"Result of job: {job.result}")
```

(note above that you will need to use the proper name for the instance
to which you want to add your user. This will be based on the agent
network that represents your infrastructure)
