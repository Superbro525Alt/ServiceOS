# Service Boundary Readiness

The kernel and root bootstrap layers are now prepared to host a broader
service-oriented system, but deferred implementation work is tracked centrally
in [docs/roadmap.md](roadmap.md), not in this page.

This document now exists only to record the stable architectural rule:

- root bootstrap and the kernel provide mechanisms
- userspace services own policy
- future expansion should refine existing service boundaries instead of pulling
  package, filesystem, networking, audio, graphics, compatibility, shell, or
  desktop policy back into the kernel

Use [docs/roadmap.md](roadmap.md) for all open follow-on work.
