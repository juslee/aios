//! IPC Kit error mapping self-test (#190), run from the ipc-timeout thread.

use crate::ipc::KernelIpc;
use crate::task::process::ProcessId;
use crate::task::ThreadId;
use shared::kits::ipc::{ChannelOps, IpcKitError, NotificationOps, SelectOps, SharedMemoryOps};
use shared::{
    Capability, ChannelId, NotificationId, RawMessage, SelectEntry, SelectKind, MAX_CHANNELS,
    MAX_MESSAGE_SIZE, MAX_NOTIFICATIONS, RING_CAPACITY,
};

use super::bad_pid::{revoke_region_access, revoke_token, token_id};
use super::select_cap::SelectCapChannels;
use super::TEST_PID;

const PAGE: usize = 4096;
const READ: u64 = 0b001;
/// `VmFlags::USER`, which no caller may set.
const USER_BIT: u64 = 0b1000;

/// `KernelIpc` reports what each wrapper knows, not the errno table's
/// default: the capability the kernel checked and the real channel id, and
/// no `CapabilityDenied` where the kernel checks no capability (#190). Each
/// case pins one mapping: either a wrapper override, where the table's
/// default differs from the expected variant, so the check fails if the
/// override is dropped; or a default the wrapper must keep, such as an empty
/// poll reading as `WouldBlock` rather than `ChannelFull`.
///
/// Not pinned here: `channel_create`'s EPERM override, which names the same
/// `ChannelCreate` as the table's placeholder, so no check can tell it from
/// the default; and the full-table overrides (`channel_create`'s ENOSPC,
/// `notification_create`'s ENOMEM), which would need the channel or
/// notification table filled during boot.
///
/// Runs in the ipc-timeout thread `my_tid` (process 1), after
/// `select_cap_test`, on that test's channels, so this test creates no
/// channel and grants no ChannelAccess. `open` (`owned_a`) is accessible and
/// empty; every message sent to it is received again, so it is left empty.
/// `owned_b` is accessible too and only named in a lookup. `denied` is not
/// accessible. The shared memory checks revoke the SharedMemoryCreate token
/// they grant right after the create, and the region's SharedMemoryAccess
/// tokens before the unmap that frees it.
///
/// The five `denied` channel cases log the kernel's usual `denied
/// ChannelAccess` warnings, and the shared memory cases its denied
/// SharedMemoryCreate / SharedMemoryAccess and `not mapped` warnings.
///
/// Logs one line: the success text, or the bitmask of failed checks (bit n
/// is check n below).
pub(super) fn kit_errors_test(my_tid: ThreadId, channels: Option<SelectCapChannels>) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => p,
        other => {
            crate::kwarn!(Ipc, "Kit-error test: wrong owner {:?}", other.map(|p| p.0));
            return;
        }
    };
    let Some(SelectCapChannels {
        owned_a: open,
        owned_b,
        denied,
    }) = channels
    else {
        crate::kwarn!(Ipc, "Kit-error test: no select-cap channels");
        return;
    };

    let mut kit = KernelIpc;
    let msg = RawMessage {
        len: 4,
        ..RawMessage::EMPTY
    };
    let chan = |id: ChannelId| SelectEntry {
        kind: SelectKind::Channel(id),
    };
    let access_denied = |id: ChannelId| {
        Some(IpcKitError::CapabilityDenied {
            required: Capability::ChannelAccess(id),
        })
    };
    let bad_id = ChannelId(MAX_CHANNELS as u32);

    let mut checks = [false; 24];

    // EPERM on a channel names ChannelAccess(id), not the table's
    // placeholder ChannelCreate. select names the channel the caller lacks.
    checks[0] = kit.send(denied, &msg).err() == access_denied(denied);
    checks[1] = kit.recv(denied, 0).err() == access_denied(denied);
    checks[2] = kit.call(denied, &msg, 10).err() == access_denied(denied);
    checks[3] = kit.select(&[chan(open), chan(denied)], 10).err() == access_denied(denied);
    // select's fallback, which no test can reach through the race behind
    // it, so the lookup is called directly: with every channel entry held
    // (a grant landed after ipc_select's check) it names the first of the
    // two channel entries, past any notification entry, and a set with no
    // channel entry gives None.
    let notification = SelectEntry {
        kind: SelectKind::Notification(NotificationId(0), 1),
    };
    checks[22] =
        crate::ipc::first_denied_channel(&[notification, chan(open), chan(owned_b)]) == Some(open);
    checks[23] = crate::ipc::first_denied_channel(&[notification]).is_none();
    // channel_destroy checks ChannelAccess before it touches the table, so
    // `denied` survives.
    checks[20] = kit.channel_destroy(denied).err() == access_denied(denied);

    // EINVAL on a channel is InvalidChannel with the real id, not
    // InvalidArgument.
    checks[4] = kit.recv(bad_id, 0).err() == Some(IpcKitError::InvalidChannel { id: bad_id });
    // EPIPE on a channel is InvalidChannel with the real id too, not the
    // table's ObjectGone, which names no id. ipc_reply checks no capability,
    // so a reply to an in-range slot that holds no channel reaches
    // channel_mut's EPIPE and logs no denial. The highest free slot is used:
    // channel_create fills the lowest, so another thread's new channel is
    // unlikely to land there between the lookup and the reply.
    let gone = crate::ipc::CHANNEL_TABLE
        .lock()
        .iter()
        .rposition(Option::is_none)
        .map(|i| ChannelId(i as u32));
    checks[19] = gone
        .is_some_and(|id| kit.reply(id, &msg).err() == Some(IpcKitError::InvalidChannel { id }));
    // A `len` past the inline buffer is the wrapper's MessageTooLarge, found
    // before any kernel call (slicing the payload by it would panic), so
    // nothing reaches `open`.
    let oversized = RawMessage {
        len: MAX_MESSAGE_SIZE + 1,
        ..RawMessage::EMPTY
    };
    checks[21] = kit.send(open, &oversized).err()
        == Some(IpcKitError::MessageTooLarge {
            size: MAX_MESSAGE_SIZE + 1,
            max: MAX_MESSAGE_SIZE,
        });

    // EAGAIN: an empty poll is WouldBlock, a full ring on send is
    // ChannelFull. A full ring on call is ENOSPC, which the table maps to
    // ResourceExhausted; the call wrapper reports ChannelFull. ipc_call
    // returns ENOSPC before it registers as the pending caller, so the call
    // neither blocks nor leaves state behind for the drain. Every message
    // sent is received again.
    let full = Some(IpcKitError::ChannelFull {
        id: open,
        capacity: RING_CAPACITY,
    });
    checks[5] = kit.recv(open, 0).err() == Some(IpcKitError::WouldBlock);
    let filled = (0..RING_CAPACITY).all(|_| kit.send(open, &msg).is_ok());
    checks[6] = filled && kit.send(open, &msg).err() == full;
    checks[7] = filled && kit.call(open, &msg, 10).err() == full;
    let drained = (0..RING_CAPACITY).all(|_| kit.recv(open, 0).is_ok());
    checks[8] = drained && kit.recv(open, 0).err() == Some(IpcKitError::WouldBlock);

    // ipc_reply with no pending call returns EPROTO: the replier's mistake,
    // not the table's NoReply.
    checks[9] = kit.reply(open, &msg).err()
        == Some(IpcKitError::InvalidArgument {
            reason: "no pending call",
        });

    // A notification id out of range is EINVAL: InvalidArgument, not
    // InvalidChannel.
    let no_notification = NotificationId(MAX_NOTIFICATIONS as u32);
    checks[10] = matches!(
        kit.signal(no_notification, 1),
        Err(IpcKitError::InvalidArgument { .. })
    );
    checks[11] = matches!(
        kit.wait(no_notification, 1, 0),
        Err(IpcKitError::InvalidArgument { .. })
    );

    let shm = shm_kit_checks(pid, &mut kit);
    checks[12..19].copy_from_slice(&shm);

    let failed = checks
        .iter()
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u32, |mask, (i, _)| mask | 1 << i);
    // A log message keeps at most 96 bytes (two ring entries); longer text
    // is cut and marked with `~`, so keep these lines short.
    if failed == 0 {
        crate::kinfo!(Ipc, "Kit-error test: variants as expected");
    } else {
        crate::kwarn!(Ipc, "Kit-error test: failed {:#x}", failed);
    }
}

/// The shared memory wrappers, on a READ-only region process `pid` (the
/// calling thread's process) creates through the Kit. Returns checks 12-18
/// of `kit_errors_test`, all false from the first one that cannot run.
fn shm_kit_checks(pid: ProcessId, kit: &mut KernelIpc) -> [bool; 7] {
    let mut checks = [false; 7];

    // Without SharedMemoryCreate: CapabilityDenied naming it. An undefined
    // flag bit is the wrapper's own decode error, before the capability
    // check. The exact reason pins the decoder: no kernel EINVAL decodes to
    // "undefined flag bits".
    let undefined_bits = Some(IpcKitError::InvalidArgument {
        reason: "undefined flag bits",
    });
    checks[0] = kit.shmem_create(PAGE, READ).err()
        == Some(IpcKitError::CapabilityDenied {
            required: Capability::SharedMemoryCreate,
        });
    checks[1] = kit.shmem_create(PAGE, USER_BIT).err() == undefined_bits;

    let create_token = crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false)
        .ok()
        .and_then(|handle| token_id(pid, handle));
    let Some(create_token) = create_token else {
        return checks;
    };
    let created = kit.shmem_create(PAGE, READ);
    revoke_token(pid, create_token);
    let Ok(region) = created else {
        return checks;
    };

    // shared_memory_unmap checks no capability: its EPERM is "not mapped",
    // a SharedMemoryError, never CapabilityDenied.
    checks[2] = kit.shmem_unmap(region).err()
        == Some(IpcKitError::SharedMemoryError {
            reason: "not mapped",
        });
    let mapped = kit.shmem_map(region, 0, READ).is_ok();
    // The creator's SharedMemoryAccess grant is revoked before the unmap
    // that frees the region, as in the other shared memory self-tests. The
    // capability is checked before the duplicate-mapping check, so the
    // repeated map fails for the missing right, naming this region.
    revoke_region_access(pid, region);
    // USER is an undefined flag bit for shmem_map too, rejected before the
    // capability check: without the decoder this map would be
    // CapabilityDenied, and with the grant still held it would be the
    // max_flags EINVAL, which decodes to a different reason.
    checks[3] = kit.shmem_map(region, 0, USER_BIT).err() == undefined_bits;
    checks[4] = mapped
        && kit.shmem_map(region, 0, READ).err()
            == Some(IpcKitError::CapabilityDenied {
                required: Capability::SharedMemoryAccess(region.0),
            });
    checks[5] = mapped && kit.shmem_unmap(region).is_ok();
    // The region is freed: EPIPE, a SharedMemoryError, not the table's
    // ObjectGone. (Another process may reuse the slot in between, which makes
    // it EPERM, "not mapped": a SharedMemoryError too.)
    checks[6] = checks[5]
        && matches!(
            kit.shmem_unmap(region),
            Err(IpcKitError::SharedMemoryError { .. })
        );
    checks
}
