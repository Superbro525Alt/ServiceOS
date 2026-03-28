# Networking Platform Services

## Scope

The current networking layer is the first durable platform slice above the
kernel packet primitive. It is intentionally small but real:

- one userspace `network-service`
- one kernel packet-interface object type
- static IPv4 addressing and default-route reporting
- static host-name resolution from a persisted resource
- ICMP echo probes for basic connectivity testing
- shell/operator commands that use the real service contract

This is not a full socket platform yet. It is the first clean network
substrate for later package transport, remote tooling, and richer service
communication.

## Service model

`network-service` owns networking policy in userspace.

It is responsible for:

- consuming one explicit packet-interface capability from the root bootstrap
- reading network configuration from `config-service`
- reading static host mappings from a storage-backed resource blob
- maintaining interface and route state
- answering operator and service requests over its public channel

It is not responsible for:

- NIC discovery policy in the kernel
- ambient access to all processes
- package repository transport policy
- GUI or desktop network configuration

## Kernel and backend boundary

The kernel exposes a generic packet-interface object. That object supports:

- interface info queries
- frame receive
- frame transmit
- blocking waits for receive readiness

The current x86_64 bring-up path implements that object with a VirtIO PCI
backend. That backend is intentionally hidden behind the packet-interface
contract so the public architecture is not QEMU-specific.

Backend placement now looks like this:

- `kernel/core/network`: packet-interface contracts and object semantics
- `platform/x86_64/qemu_virtio/net`: VirtIO PCI packet backend and IRQ wiring
- `userspace/programs/network-service`: address, route, and name-resolution
  policy

Current backend facts:

- tested under QEMU with `virtio-net-pci`
- uses legacy PCI interrupt delivery to wake packet waiters
- copies frames between kernel queues and userspace buffers

Later backends can add:

- more virtual transports
- real PCIe NIC driver hosts
- alternate packet-buffer strategies

without redesigning the public `network-service` contract.

The current Raspberry Pi 5 target only has scaffolding under
`platform/aarch64/raspi5/net` and `platform/aarch64/raspi5/rp1`. No real Pi
packet backend exists yet, and the docs are explicit about that.

## Interface and address model

The current interface model is intentionally simple:

- interfaces have a stable numeric index
- link state is reported as up/down
- each interface reports backend, MAC, MTU, IPv4 address, prefix length, and
  default gateway
- packet counters are observable for receive, transmit, and drops

The current address model is:

- one statically configured IPv4 address
- one statically configured default route
- one static host mapping file for early resolution

This is enough to establish capability-aware network access without locking the
system into an early DHCP or DNS design.

## Capability model

Network access is explicit.

- the kernel gives the root manager one bootstrap packet-interface capability
- the root manager passes it only to `network-service`
- other services do not get direct NIC or packet authority by default
- clients reach networking through `network-service` using ordinary service
  lookup permissions

This keeps raw device authority narrow while still allowing higher-level
services and shell tools to use the network through a policy-owning userspace
service.

## Operator surface

The shell currently exposes:

- `net ifaces`
- `net route`
- `net resolve <name>`
- `net ping <name|ip>`

These commands all talk to `network-service`. They do not bypass the service
graph.

## Current limitations

The current networking layer intentionally defers:

- DHCP
- DNS protocol resolution
- TCP and UDP socket services
- IPv6
- firewalling and richer network policy
- zero-copy buffer sharing
- multiple active interfaces and richer routing policy

Those are next-layer concerns on top of the current service and packet
boundaries, not reasons to redesign them.
