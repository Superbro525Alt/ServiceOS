pub(crate) const MAX_HOSTS: usize = 8;
pub(crate) const MAX_HOSTNAME_BYTES: usize = 48;
/// Longest name the resolver tracks (request names stay <= MAX_HOSTNAME_BYTES,
/// but CNAME chain targets may legally be longer).
pub(crate) const MAX_RESOLVER_NAME_BYTES: usize = 64;
pub(crate) const MAX_HOSTS_RESOURCE_BYTES: usize = 256;
pub(crate) const MAX_FRAME_BYTES: usize = 1536;
pub(crate) const MAX_TCP_SOCKETS: usize = 2;
pub(crate) const TCP_SOCKET_BUFFER_BYTES: usize = 1024;
pub(crate) const PING_IDENTIFIER: u16 = 0x534f;
pub(crate) const EPHEMERAL_PORT_BASE: u16 = 49_152;
pub(crate) const MAX_SOCKET_INLINE_BYTES: usize =
    (serviceos_userspace_runtime::IPC_MAX_WORDS - 2) * 8;

pub(crate) const MAX_UDP_SOCKETS: usize = 4;
pub(crate) const UDP_DATAGRAM_BUFFER_BYTES: usize = 2048;
pub(crate) const MAX_TCP_LISTENERS: usize = 2;
pub(crate) const TCP_ACCEPT_BACKLOG: usize = 2;

pub(crate) const LOOPBACK_ADDRESS: smoltcp::wire::Ipv4Address =
    smoltcp::wire::Ipv4Address::new(127, 0, 0, 1);

pub(crate) const SELFTEST_UDP_PORT_A: u16 = 40_123;
pub(crate) const SELFTEST_UDP_PORT_B: u16 = 40_124;
pub(crate) const SELFTEST_TCP_PORT: u16 = 40_125;
pub(crate) const SELFTEST_POLL_LIMIT: usize = 4096;

// --- Resolver cache / DNS client ---
pub(crate) const MAX_RESOLVER_CACHE_ENTRIES: usize = 16;
/// Hard bound on CNAME indirections followed for a single resolution.
pub(crate) const MAX_CNAME_CHAIN: usize = 8;
pub(crate) const MAX_CACHED_A_RECORDS: usize = 6;
/// Negative (NXDOMAIN/NODATA) entries are capped well below positive TTLs so
/// operator-side DNS fixes propagate quickly.
pub(crate) const NEGATIVE_TTL_MS_CAP: u64 = 30_000;
/// NODATA answers get an even shorter negative lifetime than NXDOMAIN.
pub(crate) const NODATA_TTL_MS: u64 = 10_000;
pub(crate) const DNS_UDP_BUFFER_BYTES: usize = 512;
pub(crate) const DNS_SERVER_PORT: u16 = 53;
pub(crate) const DNS_RETRANSMIT_MS: u64 = 1000;
/// Maximum TXT record payload kept per answer.
pub(crate) const MAX_TXT_BYTES: usize = 64;

// --- Firewall ---
// Table size is bounded so a full rules+counters dump fits one IPC message
// (4 summary words + 2 words per rule <= IPC_MAX_WORDS = 16).
pub(crate) const MAX_FIREWALL_RULES: usize = 6;

// Reserved network-contract tags handled by this service's public channel.
// These extend the NetworkTag numeric range (next free value after
// SocketListenReply = 0x80d) until the shared ABI crate promotes them; the
// wire format is the standard RawMessage tag/words/handles envelope, so a
// future shared-abi enum addition is wire-compatible.
pub(crate) const FIREWALL_RULES_SET_REQUEST: u32 = 0x80e;
pub(crate) const FIREWALL_RULES_GET_REQUEST: u32 = 0x810;
pub(crate) const FIREWALL_RULES_REPLY: u32 = 0x80f;
pub(crate) const RESOLVE_EX_REQUEST: u32 = 0x812;
pub(crate) const RESOLVE_EX_REPLY: u32 = 0x813;

/// ResolveEx query type words (DNS rdata type numbers).
pub(crate) const RESOLVE_EX_TYPE_A: u64 = 1;
pub(crate) const RESOLVE_EX_TYPE_AAAA: u64 = 28;
pub(crate) const RESOLVE_EX_TYPE_TXT: u64 = 16;

/// ResolveReply/ResolveExReply trailing "detail" codes (words[3]).
pub(crate) const RESOLVE_DETAIL_FRESH: u64 = 0;
pub(crate) const RESOLVE_DETAIL_NXDOMAIN: u64 = 1;
pub(crate) const RESOLVE_DETAIL_SERVFAIL: u64 = 2;
pub(crate) const RESOLVE_DETAIL_NODATA: u64 = 3;
pub(crate) const RESOLVE_DETAIL_TIMEOUT: u64 = 4;
pub(crate) const RESOLVE_DETAIL_NEGATIVE_CACHE: u64 = 5;
pub(crate) const RESOLVE_DETAIL_POSITIVE_CACHE: u64 = 6;
pub(crate) const RESOLVE_DETAIL_CHAIN_TOO_LONG: u64 = 7;
pub(crate) const RESOLVE_DETAIL_MALFORMED: u64 = 8;
