# OpenPortal

This is an implementation of the OpenPortal protocol for communication
between a user portal (e.g. Waldur) and digital research infrastructure
(e.g. the Isambard supercomputers provided by BriSC).

## Components

The OpenPortal protocol is implemented in two components:

1. ``paddington`` - a library used for communication between the components of
   the OpenPortal protocol, and for defining the core components,
   e.g. the provider, service and instance.
2. ``templemeads`` - a library used to provide a generic interface with
   a specific implementation of a portal (e.g. Waldur)

These help define four types of services in the system

1. ``portal-svc`` - the service that handles communication between the
   portal and the provider
2. ``provider-svc`` - the service that handles communication between the
   provider and the portal
3. ``XXX-service-svc`` - the service that manages communication with
   an offered service on the infrastructure, e.g. ``jupyter-service-svc``
   handles and manages the Juypter offering
4. ``XXX-instance-svc`` - the service that manages communication with
   an instance of a service, e.g. ``jupyter-instance-svc`` manages
   an individual Jupyter instance - there would be one of these for
   each instance of JupyterHub running on the infrastructure.
