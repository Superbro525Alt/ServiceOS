//! Whole-system update ("sysupdate") service flows: an ordered set of
//! per-package updates executed as ONE operation-journal entry with a
//! persisted per-step cursor and commit marker, full reverse-order rollback
//! through the existing per-package rollback slots, and a bounded history
//! ring. Pure decision math lives in [`crate::sysupdate_model`].

use super::*;

use crate::sysupdate_model::{
    self, HistoryRow, MAX_SYSUPDATE_STEPS, ParsedTxn, SYSUPDATE_HISTORY_REPLY_ROWS,
    TXN_STATE_APPLYING, TXN_STATE_COMMITTED, TXN_STATE_COMMITTING, TXN_STATE_FAILED,
    TXN_STATE_PLANNING, TXN_STATE_ROLLED_BACK, TXN_STATE_ROLLING_BACK,
};

/// Reply flag bits shared with the shell.
pub(crate) const SYSUPDATE_FLAG_ROLLED_BACK: u64 = 1;
pub(crate) const SYSUPDATE_FLAG_COMMITTED_TXN_PRESENT: u64 = 2;

/// Build the ordered plan: every installed package whose catalog offers a
/// newer selectable target, ascending by service id.
pub(crate) fn build_plan_ids(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    ordered: &mut [u32; MAX_SYSUPDATE_STEPS],
) -> usize {
    let mut candidates = [0u32; MAX_SYSUPDATE_STEPS];
    let mut count = 0usize;
    for slot in packages[..package_count].iter().filter(|slot| slot.occupied) {
        if slot.installed.is_none() || count >= candidates.len() {
            continue;
        }
        let has_target = matches!(
            select_update_target(slot, repos, repo_count, None, None),
            Ok(Some(_))
        );
        if has_target {
            candidates[count] = slot.service_id as u32;
            count += 1;
        }
    }
    sysupdate_model::order_ids(&candidates[..count], ordered)
}

fn load_committed_txn(storage_handle: rt::Handle) -> Option<ParsedTxn> {
    crate::storage::load_sysupdate_txn(storage_handle)
        .ok()
        .flatten()
        .filter(|txn| txn.state == TXN_STATE_COMMITTED)
}

fn monotonic_tick() -> u64 {
    rt::monotonic_now().unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_sysupdate_request(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    message: &RawMessage,
    action: u64,
) -> rt::Result<()> {
    if message.word_count < 1 || message.handle_count < 1 {
        return Ok(());
    }
    match action {
        ops_model::MAINTENANCE_ACTION_SYSUPDATE_PLAN => send_sysupdate_reply(
            message.handles[0],
            action,
            plan_status(repos, repo_count, packages, package_count),
            plan_count(repos, repo_count, packages, package_count),
            0,
            plan_flags(storage_handle),
            &[],
        ),
        ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY => handle_sysupdate_apply(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            packages,
            package_count,
            journal,
            message.handles[0],
        ),
        ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK => handle_sysupdate_rollback(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            packages,
            package_count,
            journal,
            message.handles[0],
        ),
        _ => handle_sysupdate_history(storage_handle, message.handles[0], action),
    }
}

fn plan_count(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> usize {
    let mut ordered = [0u32; MAX_SYSUPDATE_STEPS];
    build_plan_ids(repos, repo_count, packages, package_count, &mut ordered)
}

fn plan_flags(storage_handle: rt::Handle) -> u64 {
    u64::from(load_committed_txn(storage_handle).is_some())
        * SYSUPDATE_FLAG_COMMITTED_TXN_PRESENT
}

fn plan_status(
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &[PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
) -> PackageStatus {
    if plan_count(repos, repo_count, packages, package_count) == 0 {
        PackageStatus::NoChange
    } else {
        PackageStatus::Ok
    }
}

/// Reply layout (fits the 16-word IPC budget):
/// [status, action_echo, count, secondary, flags,
///  payload...] where payload packs two service ids per word for
/// plan/apply/rollback, or (tick, meta) history pairs with
/// secondary = rows-returned for the history action.
pub(crate) fn send_sysupdate_reply(
    reply_handle: rt::Handle,
    action: u64,
    status: PackageStatus,
    count: usize,
    secondary: u64,
    flags: u64,
    payload: &[u64],
) -> rt::Result<()> {
    let mut reply = RawMessage::empty(PackageTag::MaintenanceReply as u32);
    reply.words[0] = status as u32 as u64;
    reply.words[1] = action;
    reply.words[2] = count as u64;
    reply.words[3] = secondary;
    reply.words[4] = flags;
    let mut word_index = 5usize;
    for value in payload {
        if word_index >= rt::IPC_MAX_WORDS {
            break;
        }
        reply.words[word_index] = *value;
        word_index += 1;
    }
    reply.word_count = word_index as u32;
    let _ = rt::channel_send(reply_handle, &reply);
    let _ = rt::handle_close(reply_handle);
    Ok(())
}

fn pack_ids(ids: &[u32; MAX_SYSUPDATE_STEPS], count: usize, payload: &mut [u64; 11]) -> usize {
    let mut words = 0usize;
    let mut index = 0usize;
    while index < count && words + 1 <= payload.len() {
        let low = ids[index] as u64;
        let high = ids.get(index + 1).copied().unwrap_or(0) as u64;
        payload[words] = low | (high << 32);
        words += 1;
        index += 2;
    }
    words
}

#[allow(clippy::too_many_arguments)]
fn handle_sysupdate_apply(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    reply_handle: rt::Handle,
) -> rt::Result<()> {
    if journal.pending_action != JOURNAL_NONE && journal.pending_action != JOURNAL_SYSUPDATE {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY,
            PackageStatus::Busy,
            0,
            0,
            0,
            &[],
        );
    }
    let mut txn = ParsedTxn::empty();
    txn.total = build_plan_ids(
        repos,
        repo_count,
        packages,
        package_count,
        &mut txn.ids,
    ) as usize;
    txn.count = txn.total;
    if txn.total == 0 {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY,
            PackageStatus::NoChange,
            0,
            0,
            plan_flags(storage_handle),
            &[],
        );
    }
    if !sysupdate_model::txn_transition_allowed(TXN_STATE_PLANNING, TXN_STATE_APPLYING) {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY,
            PackageStatus::Unsupported,
            0,
            0,
            0,
            &[],
        );
    }
    txn.state = TXN_STATE_APPLYING;
    txn.done = 0;
    if begin_sysupdate_journal(storage_handle, journal, &txn).is_err() {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY,
            PackageStatus::Busy,
            0,
            0,
            0,
            &[],
        );
    }
    let mut progress = ops_model::ProgressTracker::new(txn.total as u32 * 5);
    progress.enter_phase(ops_model::PROGRESS_PHASE_MATERIALIZE);
    let mut payload = [0u64; 11];
    match run_apply_steps(
        bootstrap,
        storage_handle,
        network_handle,
        log_handle,
        repos,
        repo_count,
        packages,
        package_count,
        &mut txn,
        &mut progress,
    ) {
        Ok(()) => {
            let applied = txn.done;
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Info,
                LogEvent::PackageUpdated,
                applied as u64,
                0,
            );
            let words = pack_ids(&txn.ids, applied, &mut payload);
            send_sysupdate_reply(
                reply_handle,
                ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY,
                PackageStatus::Ok,
                applied,
                0,
                0,
                &payload[..words],
            )
        }
        Err(failed_at) => {
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Error,
                LogEvent::PackageActivationFailed,
                failed_at as u64,
                0,
            );
            let words = pack_ids(&txn.ids, txn.done, &mut payload);
            send_sysupdate_reply(
                reply_handle,
                ops_model::MAINTENANCE_ACTION_SYSUPDATE_APPLY,
                PackageStatus::Interrupted,
                txn.done,
                (failed_at + 1) as u64,
                0,
                &payload[..words],
            )
        }
    }
}

/// Execute apply steps `done..total`, persisting the cursor after every
/// step so an interruption resumes exactly where it stopped. Returns
/// `Err(failed_step_index)` and parks the transaction in FAILED on error.
#[allow(clippy::too_many_arguments)]
fn run_apply_steps(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    txn: &mut ParsedTxn,
    progress: &mut ops_model::ProgressTracker,
) -> Result<(), usize> {
    let mut step = txn.done;
    while step < txn.count {
        let service_id = service_id_from_word(txn.ids[step] as u64);
        let Some(index) = find_package_slot(packages, service_id, package_count) else {
            return Err(step);
        };
        let target = match select_update_target(&packages[index], repos, repo_count, None, None) {
            Ok(Some(target)) => target,
            _ => return Err(step),
        };
        let mut auto_restored = false;
        let status = crate::operations::activate_package_version(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            &mut packages[index],
            target,
            LogEvent::PackageUpdated,
            progress,
            &mut auto_restored,
        );
        if status != PackageStatus::Ok {
            txn.state = TXN_STATE_FAILED;
            txn.done = step;
            let _ = crate::storage::persist_sysupdate_txn(storage_handle, txn);
            return Err(step);
        }
        step += 1;
        txn.done = step;
        let _ = crate::storage::persist_sysupdate_txn(storage_handle, txn);
    }
    Ok(())
}

/// Commit-marker sequence after all steps ran: COMMITTING -> persisted
/// installed state -> COMMITTED -> history append -> files cleared.
#[allow(clippy::too_many_arguments)]
fn commit_transaction(
    storage_handle: rt::Handle,
    log_handle: rt::Handle,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    txn: &mut ParsedTxn,
    journal: &mut JournalState,
) -> bool {
    if !sysupdate_model::txn_transition_allowed(txn.state, TXN_STATE_COMMITTING) {
        return false;
    }
    txn.state = TXN_STATE_COMMITTING;
    if crate::storage::persist_sysupdate_txn(storage_handle, txn).is_err() {
        return false;
    }
    let _ = crate::storage::persist_installed_state(storage_handle, packages, package_count);
    if !sysupdate_model::txn_transition_allowed(TXN_STATE_COMMITTING, TXN_STATE_COMMITTED) {
        return false;
    }
    txn.state = TXN_STATE_COMMITTED;
    txn.done = txn.count;
    if crate::storage::persist_sysupdate_txn(storage_handle, txn).is_err() {
        // Committed marker lost: keep the stale journal so recovery still
        // classifies this run honestly instead of silently succeeding.
        return false;
    }
    let seq = next_history_seq(storage_handle);
    let _ = crate::storage::append_sysupdate_history(
        storage_handle,
        seq,
        monotonic_tick(),
        txn.count as u64,
        false,
    );
    let _ = emit_package_event(
        log_handle,
        LogSeverity::Info,
        LogEvent::PackageRepairCompleted,
        txn.count as u64,
        seq,
    );
    let _ = crate::storage::clear_sysupdate_txn(storage_handle);
    *journal = JournalState::empty();
    let _ = crate::storage::persist_journal_state(storage_handle, *journal);
    true
}
fn next_history_seq(storage_handle: rt::Handle) -> u64 {
    crate::storage::load_sysupdate_history(storage_handle)
        .map(|(rows, count)| {
            count
                .checked_sub(1)
                .and_then(|index| rows.get(index))
                .map(|row| row.seq + 1)
                .unwrap_or(1)
        })
        .unwrap_or(1)
}

/// Record the sysupdate journal entry plus initial transaction cursor.
fn begin_sysupdate_journal(
    storage_handle: rt::Handle,
    journal: &mut JournalState,
    txn: &ParsedTxn,
) -> rt::Result<()> {
    journal.pending_action = JOURNAL_SYSUPDATE;
    journal.service_id = ServiceId::Package;
    journal.version = InlinePath::empty();
    journal.manifest_path = InlinePath::empty();
    crate::storage::persist_journal_state(storage_handle, *journal)?;
    crate::storage::persist_sysupdate_txn(storage_handle, txn)
}

#[allow(clippy::too_many_arguments)]
fn handle_sysupdate_rollback(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
    reply_handle: rt::Handle,
) -> rt::Result<()> {
    if journal.pending_action != JOURNAL_NONE && journal.pending_action != JOURNAL_SYSUPDATE {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK,
            PackageStatus::Busy,
            0,
            0,
            0,
            &[],
        );
    }
    let Some(mut txn) = load_committed_txn(storage_handle) else {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK,
            PackageStatus::NoRollback,
            0,
            0,
            0,
            &[],
        );
    };
    if !sysupdate_model::txn_transition_allowed(txn.state, TXN_STATE_ROLLING_BACK) {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK,
            PackageStatus::Unsupported,
            0,
            0,
            0,
            &[],
        );
    }
    txn.state = TXN_STATE_ROLLING_BACK;
    txn.done = 0;
    if begin_sysupdate_journal(storage_handle, journal, &txn).is_err() {
        return send_sysupdate_reply(
            reply_handle,
            ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK,
            PackageStatus::Busy,
            0,
            0,
            0,
            &[],
        );
    }
    let mut progress = ops_model::ProgressTracker::new(txn.count as u32 * 5);
    progress.enter_phase(ops_model::PROGRESS_PHASE_MATERIALIZE);
    match run_rollback_steps(
        bootstrap,
        storage_handle,
        network_handle,
        log_handle,
        repos,
        repo_count,
        packages,
        package_count,
        &mut txn,
        &mut progress,
    ) {
        Ok(restored) => {
            txn.state = TXN_STATE_ROLLED_BACK;
            let _ = crate::storage::persist_sysupdate_txn(storage_handle, &txn);
            let seq = next_history_seq(storage_handle);
            let _ = crate::storage::append_sysupdate_history(
                storage_handle,
                seq,
                monotonic_tick(),
                restored as u64,
                true,
            );
            let _ = crate::storage::clear_sysupdate_txn(storage_handle);
            *journal = JournalState::empty();
            let _ = crate::storage::persist_journal_state(storage_handle, *journal);
            let mut payload = [0u64; 11];
            let words = pack_ids(&txn.ids, txn.count, &mut payload);
            send_sysupdate_reply(
                reply_handle,
                ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK,
                PackageStatus::Ok,
                restored,
                0,
                SYSUPDATE_FLAG_ROLLED_BACK,
                &payload[..words],
            )
        }
        Err(failed_at) => {
            let _ = emit_package_event(
                log_handle,
                LogSeverity::Error,
                LogEvent::PackageActivationFailed,
                failed_at as u64,
                0,
            );
            send_sysupdate_reply(
                reply_handle,
                ops_model::MAINTENANCE_ACTION_SYSUPDATE_ROLLBACK,
                PackageStatus::Interrupted,
                txn.done,
                (failed_at + 1) as u64,
                0,
                &[],
            )
        }
    }
}

/// Reverse-order restore through the existing per-package rollback slots.
/// Step `k` restores plan position `count-1-k`; the persisted cursor counts
/// completed reverse steps so a resumed rollback never re-restores a
/// package twice (which would re-apply its update).
#[allow(clippy::too_many_arguments)]
fn run_rollback_steps(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    txn: &mut ParsedTxn,
    progress: &mut ops_model::ProgressTracker,
) -> Result<usize, usize> {
    let mut restored = 0usize;
    let mut reverse_cursor = txn.done;
    while reverse_cursor < txn.count {
        let position = txn.count - 1 - reverse_cursor;
        let service_id = service_id_from_word(txn.ids[position] as u64);
        let slot_index = find_package_slot(packages, service_id, package_count);
        let rollback_target = slot_index.and_then(|index| packages[index].rollback);
        let Some(index) = slot_index else {
            reverse_cursor += 1;
            txn.done = reverse_cursor;
            let _ = crate::storage::persist_sysupdate_txn(storage_handle, txn);
            continue;
        };
        let Some(target) = rollback_target else {
            // Nothing to restore for this package (no prior version):
            // count the step as handled so resume moves forward.
            reverse_cursor += 1;
            txn.done = reverse_cursor;
            let _ = crate::storage::persist_sysupdate_txn(storage_handle, txn);
            continue;
        };
        let mut auto_restored = false;
        let status = crate::operations::activate_package_version(
            bootstrap,
            storage_handle,
            network_handle,
            log_handle,
            repos,
            repo_count,
            &mut packages[index],
            target,
            LogEvent::PackageRolledBack,
            progress,
            &mut auto_restored,
        );
        if status != PackageStatus::Ok {
            txn.state = TXN_STATE_FAILED;
            txn.done = reverse_cursor;
            let _ = crate::storage::persist_sysupdate_txn(storage_handle, txn);
            return Err(reverse_cursor);
        }
        restored += 1;
        reverse_cursor += 1;
        txn.done = reverse_cursor;
        let _ = crate::storage::persist_sysupdate_txn(storage_handle, txn);
    }
    Ok(restored)
}

fn handle_sysupdate_history(
    storage_handle: rt::Handle,
    reply_handle: rt::Handle,
    action: u64,
) -> rt::Result<()> {
    let (rows, count) = crate::storage::load_sysupdate_history(storage_handle)
        .unwrap_or_else(|_| sysupdate_model::parse_history_rows(""));
    let returned = count.min(SYSUPDATE_HISTORY_REPLY_ROWS);
    let mut payload = [0u64; 11];
    let mut words = 0usize;
    for row in rows[count.saturating_sub(returned)..count].iter() {
        if words + 1 > payload.len() - 1 {
            break;
        }
        let HistoryRow {
            seq: _,
            tick,
            applied,
            rolled_back,
        } = *row;
        payload[words] = tick;
        payload[words + 1] =
            applied | ((u64::from(rolled_back)) << 32);
        words += 2;
    }
    send_sysupdate_reply(
        reply_handle,
        action,
        PackageStatus::Ok,
        count,
        returned as u64,
        0,
        &payload[..words],
    )
}

/// Recovery entry point for a stale JOURNAL_SYSUPDATE journal detected at
/// startup (called from the maintenance recover flow).
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_interrupted_sysupdate(
    bootstrap: rt::Handle,
    storage_handle: rt::Handle,
    network_handle: Option<rt::Handle>,
    log_handle: rt::Handle,
    repos: &[RepositorySlot; MAX_REPOSITORIES],
    repo_count: usize,
    packages: &mut [PackageSlot; MAX_PACKAGE_SLOTS],
    package_count: usize,
    journal: &mut JournalState,
) -> (PackageStatus, u64) {
    let txn = match crate::storage::load_sysupdate_txn(storage_handle) {
        Ok(Some(txn)) => txn,
        _ => {
            // Transaction file lost or unreadable: nothing resumable.
            discard_sysupdate(storage_handle, journal);
            return (PackageStatus::Interrupted, ops_model::RECOVERY_OUTCOME_RESUME_FAILED);
        }
    };
    match txn.state {
        TXN_STATE_APPLYING if txn.done < txn.count => {
            let mut resumed = txn;
            let mut progress = ops_model::ProgressTracker::new(resumed.count as u32 * 5);
            progress.enter_phase(ops_model::PROGRESS_PHASE_MATERIALIZE);
            match run_apply_steps(
                bootstrap,
                storage_handle,
                network_handle,
                log_handle,
                repos,
                repo_count,
                packages,
                package_count,
                &mut resumed,
                &mut progress,
            ) {
                Ok(()) => {
                    if commit_transaction(
                        storage_handle,
                        log_handle,
                        packages,
                        package_count,
                        &mut resumed,
                        journal,
                    ) {
                        (PackageStatus::Ok, ops_model::RECOVERY_OUTCOME_RESUMED)
                    } else {
                        discard_sysupdate(storage_handle, journal);
                        (PackageStatus::Interrupted, ops_model::RECOVERY_OUTCOME_RESUME_FAILED)
                    }
                }
                Err(_) => {
                    discard_sysupdate(storage_handle, journal);
                    (PackageStatus::Interrupted, ops_model::RECOVERY_OUTCOME_RESUME_FAILED)
                }
            }
        }
        TXN_STATE_ROLLING_BACK if txn.done < txn.count => {
            let mut resumed = txn;
            let mut progress = ops_model::ProgressTracker::new(resumed.count as u32 * 5);
            progress.enter_phase(ops_model::PROGRESS_PHASE_MATERIALIZE);
            match run_rollback_steps(
                bootstrap,
                storage_handle,
                network_handle,
                log_handle,
                repos,
                repo_count,
                packages,
                package_count,
                &mut resumed,
                &mut progress,
            ) {
                Ok(_) => {
                    resumed.state = TXN_STATE_ROLLED_BACK;
                    let seq = next_history_seq(storage_handle);
                    let _ = crate::storage::append_sysupdate_history(
                        storage_handle,
                        seq,
                        monotonic_tick(),
                        resumed.count as u64,
                        true,
                    );
                    let _ = crate::storage::clear_sysupdate_txn(storage_handle);
                    *journal = JournalState::empty();
                    let _ = crate::storage::persist_journal_state(storage_handle, *journal);
                    (PackageStatus::Ok, ops_model::RECOVERY_OUTCOME_RESUMED)
                }
                Err(_) => {
                    discard_sysupdate(storage_handle, journal);
                    (PackageStatus::Interrupted, ops_model::RECOVERY_OUTCOME_RESUME_FAILED)
                }
            }
        }
        TXN_STATE_COMMITTED => {
            // Crash landed after the commit marker but before cleanup:
            // keep the committed transaction (rollback stays available)
            // and just release the stale journal.
            *journal = JournalState::empty();
            let _ = crate::storage::persist_journal_state(storage_handle, *journal);
            (PackageStatus::Ok, ops_model::RECOVERY_OUTCOME_RESUMED)
        }
        _ => {
            discard_sysupdate(storage_handle, journal);
            (PackageStatus::Interrupted, ops_model::RECOVERY_OUTCOME_RESUME_FAILED)
        }
    }
}

fn discard_sysupdate(storage_handle: rt::Handle, journal: &mut JournalState) {
    let _ = crate::storage::clear_sysupdate_txn(storage_handle);
    *journal = JournalState::empty();
    let _ = crate::storage::persist_journal_state(storage_handle, *journal);
}
