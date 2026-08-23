#![no_std]
#![no_main]

use rt::{
    ConfigKey, ControlTag, KernelEventKind, LOG_FILTER_ANY, LifecycleEvent, LogDomain, LogEvent,
    LogQueryStatus, LogSeverity, LogStatus, LogTag, RawMessage, ServiceId, StorageEntryKind,
};
use serviceos_userspace_runtime as rt;

const MAX_LOG_RECORDS: usize = 64;
const MAX_SUBSCRIBERS: usize = 8;
const PERSIST_MAGIC: u64 = 0x5356_4f53_4c4f_4731;
const PERSIST_WORDS: usize = 4 + (MAX_LOG_RECORDS * 9);
const PERSIST_BYTES: usize = PERSIST_WORDS * 8;
const MAX_WRITE_CHUNK: usize = (rt::IPC_MAX_WORDS - 3) * 8;
const PERSIST_FLUSH_TICKS: u64 = 10;
const PERSIST_BATCH_RECORDS: usize = 8;

#[derive(Clone, Copy)]
struct StoredRecord {
    sequence: u64,
    tick: u64,
    source: ServiceId,
    severity: LogSeverity,
    domain: LogDomain,
    event: LogEvent,
    arg0: u64,
    arg1: u64,
    arg2: u64,
}

impl StoredRecord {
    const fn empty() -> Self {
        Self {
            sequence: 0,
            tick: 0,
            source: ServiceId::RootManager,
            severity: LogSeverity::Info,
            domain: LogDomain::Bootstrap,
            event: LogEvent::ServiceStarted,
            arg0: 0,
            arg1: 0,
            arg2: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct Subscriber {
    occupied: bool,
    handle: rt::Handle,
    minimum_severity: LogSeverity,
    source_filter: u64,
    domain_filter: u64,
}

impl Subscriber {
    const fn empty() -> Self {
        Self {
            occupied: false,
            handle: rt::INVALID_HANDLE,
            minimum_severity: LogSeverity::Info,
            source_filter: LOG_FILTER_ANY,
            domain_filter: LOG_FILTER_ANY,
        }
    }
}

rt::entry!(main);

fn main() -> u64 {
    let bootstrap = 1;
    let mut startup = RawMessage::empty(0);
    if rt::channel_receive_blocking(bootstrap, &mut startup).is_err() {
        return 0xf101;
    }
    if startup.handle_count < 2 {
        return 0xf102;
    }

    let console_handle = startup.handles[0];
    let config_handle = startup.handles[1];
    let minimum_severity = match rt::config_read(config_handle, ConfigKey::LogMinimumSeverity) {
        Ok((_, value)) => value,
        Err(_) => return 0xf103,
    };
    let _ = rt::handle_close(config_handle);

    let public = match rt::channel_create() {
        Ok(pair) => pair,
        Err(_) => return 0xf104,
    };
    if rt::register_service(bootstrap, ServiceId::Log, public.second).is_err() {
        return 0xf105;
    }
    let _ = rt::handle_close(public.second);

    let _ = rt::console_write_record(
        console_handle,
        ServiceId::Log,
        LogSeverity::Info,
        LogDomain::Log,
        LogEvent::ConfigLoaded,
        minimum_severity,
        0,
        0,
    );

    let storage_handle = rt::lookup_service(bootstrap, ServiceId::Storage).ok();
    let persistence = storage_handle.and_then(setup_persistence);

    let mut records = [StoredRecord::empty(); MAX_LOG_RECORDS];
    let mut record_count = 0usize;
    let mut next_slot = 0usize;
    let mut sequence = 0u64;
    let mut subscribers = [Subscriber::empty(); MAX_SUBSCRIBERS];
    let mut next_kernel_sequence = 0u64;
    let mut persist_dirty = false;
    let mut persist_pending_records = 0usize;
    let mut last_persist_tick = rt::monotonic_now().unwrap_or(0);

    if let Some(file_handle) = persistence {
        let _ = load_records(
            file_handle,
            &mut records,
            &mut record_count,
            &mut next_slot,
            &mut sequence,
        );
    }
    if let Ok((oldest, next)) = rt::kernel_event_query_info() {
        next_kernel_sequence = if next == 0 { oldest } else { oldest.max(1) };
    }

    loop {
        let mut did_work = false;
        match poll_lifecycle(bootstrap) {
            Ok(true) => {
                if persist_dirty {
                    if let Some(file_handle) = persistence {
                        let _ = persist_records(
                            file_handle,
                            &records,
                            record_count,
                            next_slot,
                            sequence,
                        );
                    }
                }
                return 0;
            }
            Ok(false) => {}
            Err(_) => return 0xf106,
        }

        if drain_kernel_events(
            console_handle,
            minimum_severity,
            &mut records,
            &mut record_count,
            &mut next_slot,
            &mut sequence,
            &mut subscribers,
            &mut next_kernel_sequence,
            &mut persist_dirty,
            &mut persist_pending_records,
        )
        .is_err()
        {
            return 0xf107;
        }

        let mut message = RawMessage::empty(0);
        match rt::channel_receive_nonblocking(public.first, &mut message) {
            Ok(()) => {
                did_work = true;
                if handle_request(
                    &message,
                    console_handle,
                    minimum_severity,
                    &mut records,
                    &mut record_count,
                    &mut next_slot,
                    &mut sequence,
                    &mut subscribers,
                    &mut persist_dirty,
                    &mut persist_pending_records,
                )
                .is_err()
                {
                    return 0xf108;
                }
            }
            Err(rt::Error::QueueEmpty) => {}
            Err(_) => return 0xf109,
        }

        let now = rt::monotonic_now().unwrap_or(last_persist_tick);
        if persist_dirty
            && (persist_pending_records >= PERSIST_BATCH_RECORDS
                || now.saturating_sub(last_persist_tick) >= PERSIST_FLUSH_TICKS)
        {
            if let Some(file_handle) = persistence {
                if persist_records(file_handle, &records, record_count, next_slot, sequence)
                    .is_err()
                {
                    return 0xf10a;
                }
            }
            persist_dirty = false;
            persist_pending_records = 0;
            last_persist_tick = now;
            did_work = true;
        }

        if !did_work && rt::yield_current().is_err() {
            return 0xf10a;
        }
    }
}

fn handle_request(
    message: &RawMessage,
    console_handle: rt::Handle,
    minimum_severity: u64,
    records: &mut [StoredRecord; MAX_LOG_RECORDS],
    record_count: &mut usize,
    next_slot: &mut usize,
    sequence: &mut u64,
    subscribers: &mut [Subscriber; MAX_SUBSCRIBERS],
    persist_dirty: &mut bool,
    persist_pending_records: &mut usize,
) -> rt::Result<()> {
    match message.tag {
        x if x == LogTag::Record as u32 => {
            if message.word_count < 6 {
                return Ok(());
            }
            if message.words[1] < minimum_severity {
                return Ok(());
            }

            let tick = rt::monotonic_now().unwrap_or(0);
            let record = StoredRecord {
                sequence: sequence.saturating_add(1),
                tick,
                source: service_id_from_word(message.words[0]),
                severity: severity_from_word(message.words[1]),
                domain: domain_from_word(message.words[2]),
                event: event_from_word(message.words[3]),
                arg0: message.words[4],
                arg1: message.words[5],
                arg2: message.words.get(6).copied().unwrap_or(0),
            };
            append_record(
                console_handle,
                records,
                record_count,
                next_slot,
                sequence,
                subscribers,
                record,
                persist_dirty,
                persist_pending_records,
            )?;
        }
        x if x == LogTag::QueryInfoRequest as u32 => {
            if message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(LogTag::QueryInfoReply as u32);
            reply.word_count = 2;
            reply.words[0] = oldest_sequence(*sequence, *record_count);
            reply.words[1] = if *record_count == 0 {
                0
            } else {
                sequence.saturating_add(1)
            };
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == LogTag::QueryRecordRequest as u32 => {
            if message.word_count < 1 || message.handle_count < 1 {
                return Ok(());
            }
            let reply_handle = message.handles[0];
            let mut reply = RawMessage::empty(LogTag::QueryRecordReply as u32);
            reply.word_count = 2;
            reply.words[0] = LogQueryStatus::NotFound as u32 as u64;
            reply.words[1] = message.words[0];

            if let Some(record) = find_record(records, *record_count, message.words[0]) {
                reply.word_count = 10;
                reply.words[0] = LogQueryStatus::Ok as u32 as u64;
                reply.words[1] = record.sequence;
                reply.words[2] = record.tick;
                reply.words[3] = record.source as u32 as u64;
                reply.words[4] = record.severity as u32 as u64;
                reply.words[5] = record.domain as u32 as u64;
                reply.words[6] = record.event as u32 as u64;
                reply.words[7] = record.arg0;
                reply.words[8] = record.arg1;
                reply.words[9] = record.arg2;
            }

            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        x if x == LogTag::SubscribeRequest as u32 => {
            if message.handle_count < 2 {
                return Ok(());
            }
            let subscriber_handle = message.handles[0];
            let reply_handle = message.handles[1];
            let mut reply = RawMessage::empty(LogTag::SubscribeReply as u32);
            reply.word_count = 1;
            if let Some(index) = subscribers.iter().position(|entry| !entry.occupied) {
                subscribers[index] = Subscriber {
                    occupied: true,
                    handle: subscriber_handle,
                    minimum_severity: severity_from_word(message.words[0]),
                    source_filter: message.words[1],
                    domain_filter: message.words[2],
                };
                reply.words[0] = LogStatus::Ok as u32 as u64;
            } else {
                reply.words[0] = LogStatus::Busy as u32 as u64;
                let _ = rt::handle_close(subscriber_handle);
            }
            let _ = rt::channel_send(reply_handle, &reply);
            let _ = rt::handle_close(reply_handle);
        }
        _ => {}
    }

    Ok(())
}

fn append_record(
    console_handle: rt::Handle,
    records: &mut [StoredRecord; MAX_LOG_RECORDS],
    record_count: &mut usize,
    next_slot: &mut usize,
    sequence: &mut u64,
    subscribers: &mut [Subscriber; MAX_SUBSCRIBERS],
    mut record: StoredRecord,
    persist_dirty: &mut bool,
    persist_pending_records: &mut usize,
) -> rt::Result<()> {
    *sequence = sequence.saturating_add(1);
    record.sequence = *sequence;
    records[*next_slot] = record;
    *next_slot = (*next_slot + 1) % records.len();
    *record_count = (*record_count + 1).min(records.len());

    let _ = rt::console_write_record(
        console_handle,
        record.source,
        record.severity,
        record.domain,
        record.event,
        record.arg0,
        record.arg1,
        record.sequence,
    );

    emit_to_subscribers(subscribers, record);
    *persist_dirty = true;
    *persist_pending_records = persist_pending_records.saturating_add(1);
    Ok(())
}

fn emit_to_subscribers(subscribers: &mut [Subscriber; MAX_SUBSCRIBERS], record: StoredRecord) {
    let mut message = RawMessage::empty(LogTag::StreamRecord as u32);
    message.word_count = 9;
    message.words[0] = record.sequence;
    message.words[1] = record.tick;
    message.words[2] = record.source as u32 as u64;
    message.words[3] = record.severity as u32 as u64;
    message.words[4] = record.domain as u32 as u64;
    message.words[5] = record.event as u32 as u64;
    message.words[6] = record.arg0;
    message.words[7] = record.arg1;
    message.words[8] = record.arg2;

    for subscriber in subscribers.iter_mut().filter(|entry| entry.occupied) {
        if record.severity < subscriber.minimum_severity {
            continue;
        }
        if subscriber.source_filter != LOG_FILTER_ANY
            && subscriber.source_filter != record.source as u32 as u64
        {
            continue;
        }
        if subscriber.domain_filter != LOG_FILTER_ANY
            && subscriber.domain_filter != record.domain as u32 as u64
        {
            continue;
        }
        if rt::channel_send(subscriber.handle, &message).is_err() {
            let _ = rt::handle_close(subscriber.handle);
            *subscriber = Subscriber::empty();
        }
    }
}

fn drain_kernel_events(
    console_handle: rt::Handle,
    minimum_severity: u64,
    records: &mut [StoredRecord; MAX_LOG_RECORDS],
    record_count: &mut usize,
    next_slot: &mut usize,
    sequence: &mut u64,
    subscribers: &mut [Subscriber; MAX_SUBSCRIBERS],
    next_kernel_sequence: &mut u64,
    persist_dirty: &mut bool,
    persist_pending_records: &mut usize,
) -> rt::Result<()> {
    let (oldest, next) = rt::kernel_event_query_info()?;
    if *next_kernel_sequence == 0 {
        *next_kernel_sequence = oldest;
    }
    if *next_kernel_sequence < oldest {
        *next_kernel_sequence = oldest;
    }
    while *next_kernel_sequence != 0 && *next_kernel_sequence < next {
        let Some(event) = rt::kernel_event_query_record(*next_kernel_sequence)? else {
            *next_kernel_sequence = next;
            break;
        };
        *next_kernel_sequence = next_kernel_sequence.saturating_add(1);
        if event.kind != KernelEventKind::Trap || (LogSeverity::Error as u64) < minimum_severity {
            continue;
        }
        let record = StoredRecord {
            sequence: 0,
            tick: event.tick,
            source: ServiceId::RootManager,
            severity: LogSeverity::Error,
            domain: LogDomain::Kernel,
            event: LogEvent::KernelTrap,
            arg0: event.detail0 | (event.detail1 << 32) | ((event.detail4 & 0xffff_ffff) << 16),
            arg1: event.detail2,
            arg2: event.detail3,
        };
        append_record(
            console_handle,
            records,
            record_count,
            next_slot,
            sequence,
            subscribers,
            record,
            persist_dirty,
            persist_pending_records,
        )?;
    }
    Ok(())
}

fn poll_lifecycle(bootstrap: rt::Handle) -> rt::Result<bool> {
    let mut message = RawMessage::empty(0);
    match rt::channel_receive_nonblocking(bootstrap, &mut message) {
        Ok(()) if message.tag == ControlTag::Lifecycle as u32 && message.word_count > 0 => {
            Ok(matches!(
                lifecycle_event_from_word(message.words[0]),
                LifecycleEvent::Restarting | LifecycleEvent::Stopped
            ))
        }
        Ok(()) => Ok(false),
        Err(rt::Error::QueueEmpty) => Ok(false),
        Err(error) => Err(error),
    }
}

fn setup_persistence(storage_handle: rt::Handle) -> Option<rt::Handle> {
    let root = rt::storage_open_directory(storage_handle, "", true).ok()?;
    let state = ensure_child_directory(root, "state")?;
    let _ = rt::handle_close(root);
    let log_dir = ensure_child_directory(state, "log")?;
    let _ = rt::handle_close(state);
    let result = rt::storage_directory_open_file(log_dir, "records.bin", true, true).ok();
    let _ = rt::handle_close(log_dir);
    let _ = rt::handle_close(storage_handle);
    result.map(|(handle, _)| handle)
}

fn ensure_child_directory(parent: rt::Handle, name: &str) -> Option<rt::Handle> {
    match rt::storage_directory_open_path(parent, name, true) {
        Ok(handle) => Some(handle),
        Err(rt::Error::NotFound) => {
            let _ = rt::storage_directory_create(parent, name, StorageEntryKind::Directory);
            rt::storage_directory_open_path(parent, name, true).ok()
        }
        Err(_) => None,
    }
}

fn load_records(
    file_handle: rt::Handle,
    records: &mut [StoredRecord; MAX_LOG_RECORDS],
    record_count: &mut usize,
    next_slot: &mut usize,
    sequence: &mut u64,
) -> rt::Result<()> {
    let mut buffer = [0u8; PERSIST_BYTES];
    let loaded = rt::storage_read_all(file_handle, &mut buffer, PERSIST_BYTES)?;
    if loaded < 32 {
        return Ok(());
    }
    if decode_u64(&buffer, 0) != PERSIST_MAGIC {
        return Ok(());
    }
    *record_count = decode_u64(&buffer, 8) as usize;
    *next_slot = decode_u64(&buffer, 16) as usize % records.len();
    *sequence = decode_u64(&buffer, 24);
    let count = (*record_count).min(records.len());
    for index in 0..count {
        let base = 32 + index * 72;
        records[index] = StoredRecord {
            sequence: decode_u64(&buffer, base),
            tick: decode_u64(&buffer, base + 8),
            source: service_id_from_word(decode_u64(&buffer, base + 16)),
            severity: severity_from_word(decode_u64(&buffer, base + 24)),
            domain: domain_from_word(decode_u64(&buffer, base + 32)),
            event: event_from_word(decode_u64(&buffer, base + 40)),
            arg0: decode_u64(&buffer, base + 48),
            arg1: decode_u64(&buffer, base + 56),
            arg2: decode_u64(&buffer, base + 64),
        };
    }
    *record_count = count;
    Ok(())
}

fn persist_records(
    file_handle: rt::Handle,
    records: &[StoredRecord; MAX_LOG_RECORDS],
    record_count: usize,
    next_slot: usize,
    sequence: u64,
) -> rt::Result<()> {
    let mut buffer = [0u8; PERSIST_BYTES];
    encode_u64(&mut buffer, 0, PERSIST_MAGIC);
    encode_u64(&mut buffer, 8, record_count as u64);
    encode_u64(&mut buffer, 16, next_slot as u64);
    encode_u64(&mut buffer, 24, sequence);
    for (index, record) in records.iter().copied().enumerate() {
        let base = 32 + index * 72;
        encode_u64(&mut buffer, base, record.sequence);
        encode_u64(&mut buffer, base + 8, record.tick);
        encode_u64(&mut buffer, base + 16, record.source as u32 as u64);
        encode_u64(&mut buffer, base + 24, record.severity as u32 as u64);
        encode_u64(&mut buffer, base + 32, record.domain as u32 as u64);
        encode_u64(&mut buffer, base + 40, record.event as u32 as u64);
        encode_u64(&mut buffer, base + 48, record.arg0);
        encode_u64(&mut buffer, base + 56, record.arg1);
        encode_u64(&mut buffer, base + 64, record.arg2);
    }
    let mut offset = 0usize;
    while offset < buffer.len() {
        let chunk_len = (buffer.len() - offset).min(MAX_WRITE_CHUNK);
        let _ = rt::storage_write(
            file_handle,
            offset,
            buffer.len(),
            &buffer[offset..offset + chunk_len],
        )?;
        offset += chunk_len;
    }
    Ok(())
}

fn encode_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn decode_u64(buffer: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&buffer[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

fn oldest_sequence(next_sequence: u64, record_count: usize) -> u64 {
    if record_count == 0 {
        0
    } else {
        next_sequence
            .saturating_sub(record_count as u64)
            .saturating_add(1)
    }
}

fn find_record(
    records: &[StoredRecord; MAX_LOG_RECORDS],
    record_count: usize,
    sequence: u64,
) -> Option<StoredRecord> {
    records[..record_count]
        .iter()
        .copied()
        .find(|record| record.sequence == sequence)
}

fn service_id_from_word(value: u64) -> ServiceId {
    match value as u32 {
        x if x == ServiceId::Storage as u32 => ServiceId::Storage,
        x if x == ServiceId::Console as u32 => ServiceId::Console,
        x if x == ServiceId::Config as u32 => ServiceId::Config,
        x if x == ServiceId::Log as u32 => ServiceId::Log,
        x if x == ServiceId::Status as u32 => ServiceId::Status,
        x if x == ServiceId::Shell as u32 => ServiceId::Shell,
        x if x == ServiceId::Package as u32 => ServiceId::Package,
        x if x == ServiceId::Announce as u32 => ServiceId::Announce,
        x if x == ServiceId::Network as u32 => ServiceId::Network,
        x if x == ServiceId::Graphics as u32 => ServiceId::Graphics,
        x if x == ServiceId::Session as u32 => ServiceId::Session,
        x if x == ServiceId::DesktopShell as u32 => ServiceId::DesktopShell,
        x if x == ServiceId::Terminal as u32 => ServiceId::Terminal,
        x if x == ServiceId::Audio as u32 => ServiceId::Audio,
        x if x == ServiceId::Runtime as u32 => ServiceId::Runtime,
        x if x == ServiceId::Developer as u32 => ServiceId::Developer,
        x if x == ServiceId::Clipboard as u32 => ServiceId::Clipboard,
        x if x == ServiceId::Security as u32 => ServiceId::Security,
        _ => ServiceId::RootManager,
    }
}

fn severity_from_word(value: u64) -> LogSeverity {
    match value as u32 {
        x if x == LogSeverity::Trace as u32 => LogSeverity::Trace,
        x if x == LogSeverity::Debug as u32 => LogSeverity::Debug,
        x if x == LogSeverity::Warn as u32 => LogSeverity::Warn,
        x if x == LogSeverity::Error as u32 => LogSeverity::Error,
        _ => LogSeverity::Info,
    }
}

fn domain_from_word(value: u64) -> LogDomain {
    match value as u32 {
        x if x == LogDomain::Bootstrap as u32 => LogDomain::Bootstrap,
        x if x == LogDomain::ServiceManager as u32 => LogDomain::ServiceManager,
        x if x == LogDomain::Storage as u32 => LogDomain::Storage,
        x if x == LogDomain::Log as u32 => LogDomain::Log,
        x if x == LogDomain::Config as u32 => LogDomain::Config,
        x if x == LogDomain::Console as u32 => LogDomain::Console,
        x if x == LogDomain::Status as u32 => LogDomain::Status,
        x if x == LogDomain::Ipc as u32 => LogDomain::Ipc,
        x if x == LogDomain::Shell as u32 => LogDomain::Shell,
        x if x == LogDomain::Package as u32 => LogDomain::Package,
        x if x == LogDomain::Network as u32 => LogDomain::Network,
        x if x == LogDomain::Graphics as u32 => LogDomain::Graphics,
        x if x == LogDomain::Session as u32 => LogDomain::Session,
        x if x == LogDomain::Desktop as u32 => LogDomain::Desktop,
        x if x == LogDomain::App as u32 => LogDomain::App,
        x if x == LogDomain::Audio as u32 => LogDomain::Audio,
        x if x == LogDomain::Runtime as u32 => LogDomain::Runtime,
        x if x == LogDomain::Developer as u32 => LogDomain::Developer,
        x if x == LogDomain::Security as u32 => LogDomain::Security,
        x if x == LogDomain::Kernel as u32 => LogDomain::Kernel,
        _ => LogDomain::Service,
    }
}

fn event_from_word(value: u64) -> LogEvent {
    match value as u32 {
        x if x == LogEvent::ServiceStarted as u32 => LogEvent::ServiceStarted,
        x if x == LogEvent::ServiceReady as u32 => LogEvent::ServiceReady,
        x if x == LogEvent::ServiceFailed as u32 => LogEvent::ServiceFailed,
        x if x == LogEvent::ServiceRestarting as u32 => LogEvent::ServiceRestarting,
        x if x == LogEvent::ConfigLoaded as u32 => LogEvent::ConfigLoaded,
        x if x == LogEvent::ConfigRead as u32 => LogEvent::ConfigRead,
        x if x == LogEvent::ConsoleWrite as u32 => LogEvent::ConsoleWrite,
        x if x == LogEvent::StatusStarted as u32 => LogEvent::StatusStarted,
        x if x == LogEvent::StatusHeartbeat as u32 => LogEvent::StatusHeartbeat,
        x if x == LogEvent::LookupGranted as u32 => LogEvent::LookupGranted,
        x if x == LogEvent::StorageMounted as u32 => LogEvent::StorageMounted,
        x if x == LogEvent::ManifestLoaded as u32 => LogEvent::ManifestLoaded,
        x if x == LogEvent::ResourceOpened as u32 => LogEvent::ResourceOpened,
        x if x == LogEvent::SessionOpened as u32 => LogEvent::SessionOpened,
        x if x == LogEvent::ShellCommand as u32 => LogEvent::ShellCommand,
        x if x == LogEvent::ToolLaunched as u32 => LogEvent::ToolLaunched,
        x if x == LogEvent::PackageCatalogLoaded as u32 => LogEvent::PackageCatalogLoaded,
        x if x == LogEvent::PackageInstalled as u32 => LogEvent::PackageInstalled,
        x if x == LogEvent::PackageUpdated as u32 => LogEvent::PackageUpdated,
        x if x == LogEvent::PackageRemoved as u32 => LogEvent::PackageRemoved,
        x if x == LogEvent::PackageRolledBack as u32 => LogEvent::PackageRolledBack,
        x if x == LogEvent::PackageActivationFailed as u32 => LogEvent::PackageActivationFailed,
        x if x == LogEvent::NetworkInterfaceReady as u32 => LogEvent::NetworkInterfaceReady,
        x if x == LogEvent::NetworkAddressConfigured as u32 => LogEvent::NetworkAddressConfigured,
        x if x == LogEvent::NetworkResolveCompleted as u32 => LogEvent::NetworkResolveCompleted,
        x if x == LogEvent::NetworkProbeCompleted as u32 => LogEvent::NetworkProbeCompleted,
        x if x == LogEvent::NetworkLinkChanged as u32 => LogEvent::NetworkLinkChanged,
        x if x == LogEvent::DisplayOutputReady as u32 => LogEvent::DisplayOutputReady,
        x if x == LogEvent::SurfaceCreated as u32 => LogEvent::SurfaceCreated,
        x if x == LogEvent::SurfaceUpdated as u32 => LogEvent::SurfaceUpdated,
        x if x == LogEvent::CompositorPresented as u32 => LogEvent::CompositorPresented,
        x if x == LogEvent::SessionReady as u32 => LogEvent::SessionReady,
        x if x == LogEvent::SessionFocusChanged as u32 => LogEvent::SessionFocusChanged,
        x if x == LogEvent::DesktopReady as u32 => LogEvent::DesktopReady,
        x if x == LogEvent::DesktopAppLaunched as u32 => LogEvent::DesktopAppLaunched,
        x if x == LogEvent::DesktopAppExited as u32 => LogEvent::DesktopAppExited,
        x if x == LogEvent::DesktopFocusChanged as u32 => LogEvent::DesktopFocusChanged,
        x if x == LogEvent::AppRendered as u32 => LogEvent::AppRendered,
        x if x == LogEvent::InputSourceReady as u32 => LogEvent::InputSourceReady,
        x if x == LogEvent::InputKeyDelivered as u32 => LogEvent::InputKeyDelivered,
        x if x == LogEvent::NetworkLeaseChanged as u32 => LogEvent::NetworkLeaseChanged,
        x if x == LogEvent::NetworkSocketOpened as u32 => LogEvent::NetworkSocketOpened,
        x if x == LogEvent::NetworkSocketClosed as u32 => LogEvent::NetworkSocketClosed,
        x if x == LogEvent::TerminalSessionOpened as u32 => LogEvent::TerminalSessionOpened,
        x if x == LogEvent::TerminalSessionClosed as u32 => LogEvent::TerminalSessionClosed,
        x if x == LogEvent::AudioEndpointReady as u32 => LogEvent::AudioEndpointReady,
        x if x == LogEvent::AudioStreamOpened as u32 => LogEvent::AudioStreamOpened,
        x if x == LogEvent::AudioStreamStarted as u32 => LogEvent::AudioStreamStarted,
        x if x == LogEvent::AudioStreamStopped as u32 => LogEvent::AudioStreamStopped,
        x if x == LogEvent::AudioStreamClosed as u32 => LogEvent::AudioStreamClosed,
        x if x == LogEvent::RuntimeEnvironmentCreated as u32 => LogEvent::RuntimeEnvironmentCreated,
        x if x == LogEvent::RuntimeEnvironmentDestroyed as u32 => {
            LogEvent::RuntimeEnvironmentDestroyed
        }
        x if x == LogEvent::RuntimeLaunchStarted as u32 => LogEvent::RuntimeLaunchStarted,
        x if x == LogEvent::RuntimeLaunchExited as u32 => LogEvent::RuntimeLaunchExited,
        x if x == LogEvent::RuntimeMappedRead as u32 => LogEvent::RuntimeMappedRead,
        x if x == LogEvent::DeveloperCatalogLoaded as u32 => LogEvent::DeveloperCatalogLoaded,
        x if x == LogEvent::DeveloperBuildStarted as u32 => LogEvent::DeveloperBuildStarted,
        x if x == LogEvent::DeveloperBuildFinished as u32 => LogEvent::DeveloperBuildFinished,
        x if x == LogEvent::DeveloperBuildFailed as u32 => LogEvent::DeveloperBuildFailed,
        x if x == LogEvent::DeveloperArtifactOpened as u32 => LogEvent::DeveloperArtifactOpened,
        x if x == LogEvent::PackageRepositoryAdded as u32 => LogEvent::PackageRepositoryAdded,
        x if x == LogEvent::PackageRepositorySynced as u32 => LogEvent::PackageRepositorySynced,
        x if x == LogEvent::PackageRepositorySyncFailed as u32 => {
            LogEvent::PackageRepositorySyncFailed
        }
        x if x == LogEvent::PackageRepairCompleted as u32 => LogEvent::PackageRepairCompleted,
        x if x == LogEvent::PackageGarbageCollected as u32 => LogEvent::PackageGarbageCollected,
        x if x == LogEvent::SecurityPolicyChanged as u32 => LogEvent::SecurityPolicyChanged,
        x if x == LogEvent::SecurityLaunchDenied as u32 => LogEvent::SecurityLaunchDenied,
        x if x == LogEvent::RuntimeApprovalPending as u32 => LogEvent::RuntimeApprovalPending,
        x if x == LogEvent::RuntimeApprovalChanged as u32 => LogEvent::RuntimeApprovalChanged,
        x if x == LogEvent::KernelTrap as u32 => LogEvent::KernelTrap,
        _ => LogEvent::LookupGranted,
    }
}

fn lifecycle_event_from_word(value: u64) -> LifecycleEvent {
    match value as u32 {
        x if x == LifecycleEvent::Starting as u32 => LifecycleEvent::Starting,
        x if x == LifecycleEvent::Ready as u32 => LifecycleEvent::Ready,
        x if x == LifecycleEvent::Failed as u32 => LifecycleEvent::Failed,
        x if x == LifecycleEvent::Stopped as u32 => LifecycleEvent::Stopped,
        _ => LifecycleEvent::Restarting,
    }
}
