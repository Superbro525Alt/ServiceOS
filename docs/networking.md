# Networking Platform Services

## Scope

The current networking layer is now the first practical networking platform
slice above the kernel packet primitive. It is still intentionally modest, but
it is no longer just static bring-up plumbing:

- one userspace `network-service`
- one kernel packet-interface object type
- dynamic IPv4 configuration with a static fallback path
- DNS-backed host resolution with simple static-host overrides
- ICMP echo probes for reachability testing
- outbound TCP stream sessions for client-style connectivity
- shell/operator commands that use the real service contract

This is still not a full internet subsystem. It is the first clean, usable
network substrate for package transport, remote tooling, and later richer
service communication.

## Service model

`network-service` owns networking policy in userspace.

It is responsible for:

- consuming one explicit packet-interface capability from the root bootstrap
- reading network configuration from `config-service`
- reading static host mappings from a storage-backed resource blob
- maintaining interface, route, resolver, and transport state
- acquiring or falling back to IPv4 configuration
- exposing resolution, probe, and stream-transport operations over its public
  channel

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
- one-shot receive-side backend polling when the queue is empty, so practical
  networking does not collapse if one interrupt edge is missed

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
- remains backend-agnostic at the `network-service` boundary

Later backends can add:

- more virtual transports
- real PCIe NIC driver hosts
- alternate packet-buffer strategies

without redesigning the public `network-service` contract.

The current Raspberry Pi 5 target only has scaffolding under
`platform/aarch64/raspi5/net` and `platform/aarch64/raspi5/rp1`. No real Pi
packet backend exists yet, and the docs are explicit about that.

## Interface and address model

The interface model remains intentionally simple:

- interfaces have a stable numeric index
- link state is reported as up/down
- each interface reports backend, MAC, MTU, config mode, config state, IPv4
  address, prefix length, default gateway, and active DNS server
- packet counters are observable for receive, transmit, and drops

The current address model is:

- one active IPv4 address, prefix, gateway, and DNS server per interface
- dynamic acquisition via DHCPv4 when enabled
- static fallback when DHCP acquisition times out
- one static host mapping file for pinned early aliases and overrides

Configuration state is explicit:

- `static/configured`
- `dynamic/pending`
- `dynamic/configured`
- `dynamic/fallback-static`
- `dynamic/failed`

The current QEMU/VirtIO path successfully reaches `dynamic/configured` under
the usual user-mode network, but the public contract does not depend on that
backend.

## Resolution model

Host resolution stays inside `network-service`.

Resolution order is:

1. literal IPv4
2. static host alias from the bundled host resource
3. DNS A-record query through the active resolver server

The resolver currently:

- uses the interface DNS server from DHCP or static config
- performs simple request/response lookups without a separate cache daemon
- returns IPv4 results through the existing `Resolve` service contract

That keeps DNS out of clients while avoiding a second service split before it
is justified.

## Transport model

The current transport surface is a small but real client API.

`network-service` now provides:

- interface listing and status
- route/default-path reporting
- DNS-backed host resolution
- ICMP echo probing
- outbound TCP stream session open/list/status/send/receive/close

The transport boundary is capability-aware:

- clients look up `network-service`
- clients do not receive raw NIC or packet authority
- a TCP stream session is represented as a dedicated per-session channel handle
- the operator shell uses the same service and session handles as any other
  client

This is intentionally not POSIX sockets. It is a narrower service-native
transport surface that future UDP, listeners, and richer connection APIs can
grow from without changing the kernel packet contract.

## Capability model

Network access is explicit.

- the kernel gives the root manager one bootstrap packet-interface capability
- the root manager passes it only to `network-service`
- other services do not get direct NIC or packet authority by default
- clients reach networking through `network-service` using ordinary service
  lookup permissions
- transport authority is therefore service-mediated rather than ambient

This keeps raw device authority narrow while still allowing higher-level
services and shell tools to use the network through a policy-owning userspace
service.

## Operator surface

The shell currently exposes:

- `net ifaces`
- `net route`
- `net sockets`
- `net resolve <name>`
- `net ping <name|ip>`
- `net http <host> [path]`

These commands all talk to `network-service`. They do not bypass the service
graph.

The current end-to-end operator workflow on `qemu-virtio` is:

1. `network-service` comes up with a packet-interface capability
2. DHCP configures `10.0.2.15/24`, gateway `10.0.2.2`, and DNS `10.0.2.3`
3. `net ifaces` shows dynamic/configured state and live packet counters
4. `net resolve example.com` returns a DNS-backed address
5. `net ping gateway` succeeds through the ICMP service path
6. `net http example.com /` opens a real TCP stream through `network-service`
   and returns an HTTP response

## Current limitations

The networking layer still intentionally defers:

- UDP client transport
- listening sockets and inbound service transports
- IPv6
- richer routing and multi-interface policy
- long-lived resolver caching and richer DNS record support
- firewalling and richer network policy
- zero-copy buffer sharing
- backend-native real NIC support beyond the current QEMU/VirtIO path

Those are next-layer concerns on top of the current service and packet
boundaries, not reasons to redesign them.
