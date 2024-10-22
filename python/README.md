<!--
# SPDX-FileCopyrightText: © 2024 Christopher Woods <Christopher.Woods@bristol.ac.uk>
# SPDX-License-Identifier: CC0-1.0
-->

# Developing the Python wrappers

You should create a Python virtual environment into which to develop / install the wrappers.

For example;

```
$ python -m venv .env
$ source .env/bin/activate
$ pip install maturin
```

Then, you can change into this directory and run

```
$ maturin develop
```

to compile and install the openportal wrappers into this environment.

