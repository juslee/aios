//! Create-ABI self-test (#163), run from the ipc-timeout thread: SVC 6
//! (ChannelCreate) and SVC 20 (SharedMemoryCreate) return the new id in x0
//! and, in x1, the handle of the access token minted for the creator as a
//! child of the authorising create token.

use crate::arch::aarch64::trap::TrapFrame;
use crate::cap::revoke_in_process;
use crate::ipc::{channel_destroy, channel_mut, shmem, CHANNEL_TABLE};
use crate::mm::pgtable::VmFlags;
use crate::syscall::IpcError;
use crate::task::process::{process_ref, ProcessId, PROCESS_TABLE};
use crate::task::ThreadId;
use shared::{
    Capability, CapabilityHandle, CapabilityToken, CapabilityTokenId, ChannelId, SharedMemoryId,
    Syscall,
};

use super::bad_pid::token_id;
use super::TEST_PID;

/// No valid channel or region id is below 256: the generation sits in bits
/// 8-31 and is never 0 for an issued id. An errno is negative.
const MIN_VALID_ID: i64 = 1 << shared::ID_SLOT_BITS;

/// SharedMemoryCreate's flags: READ | WRITE (`VmFlags::from_caller_bits`).
const READ_WRITE: u64 = 0b011;

/// SVC 15's `new_cap_type` for ChannelAccess and SharedMemoryAccess.
const ATTENUATE_CHANNEL_ACCESS: u64 = 1;
const ATTENUATE_SHM_ACCESS: u64 = 3;

/// ChannelCreate and SharedMemoryCreate through `syscall_dispatch`, the path
/// an EL0 SVC takes, in the ipc-timeout thread `my_tid` (process 1).
///
/// ChannelCreate: x0 is an id of 256 or more, and x1 names a live
/// `ChannelAccess(x0)` in process 1's table whose `parent_token` is process
/// 1's ChannelCreate token; the channel records process 1 as its `creator`
/// and that token as its `creation_cap`. CapabilityAttenuate type 1
/// (ChannelAccess(x0)) from the ChannelCreate handle returns EPERM. The
/// channel is then destroyed with the minted token; the token stays, stale,
/// like the Destroy test's.
///
/// SharedMemoryCreate, the same, under a temporary SharedMemoryCreate grant
/// T: on entry process 1 must hold no live SharedMemoryCreate (otherwise it
/// logs `holds SharedMemoryCreate` and stops), so T is the only token the
/// create can use. The region's `creation_cap` is T, and so is the
/// `parent_token` of the token x1 names. CapabilityAttenuate type 3 from T's
/// handle returns EPERM. The region is freed by a map and an unmap, and T is
/// revoked through `revoke_in_process` before the test returns, which also
/// revokes the minted token, its child.
///
/// No lock is held across a syscall or IPC call. Logs one line: `Create-ABI
/// test: x0/x1 as expected`, or the bitmask of failed checks (bit n is check
/// n; channel checks 0-4, shared memory checks 5-10).
pub(super) fn create_abi_test(my_tid: ThreadId) {
    let pid = match crate::cap::process_of_thread(my_tid) {
        Some(p) if p == TEST_PID => p,
        other => {
            crate::kwarn!(Ipc, "Create-ABI test: wrong owner {:?}", other.map(|p| p.0));
            return;
        }
    };
    if live_token(pid, &Capability::SharedMemoryCreate).is_some() {
        crate::kwarn!(Ipc, "Create-ABI test: holds SharedMemoryCreate");
        return;
    }

    let channel = channel_checks(pid);
    let shm = shm_checks(pid);

    let failed = channel
        .iter()
        .chain(shm.iter())
        .enumerate()
        .filter(|(_, ok)| !**ok)
        .fold(0u32, |mask, (i, _)| mask | 1 << i);
    // A log message keeps at most 96 bytes (two ring entries); longer text
    // is cut and marked with `~`, so keep these lines short.
    if failed == 0 {
        crate::kinfo!(Ipc, "Create-ABI test: x0/x1 as expected");
    } else {
        crate::kwarn!(Ipc, "Create-ABI test: failed {:#x}", failed);
    }
}

/// The ChannelCreate checks, 0-4 of `create_abi_test`. All false if process
/// 1 holds no live ChannelCreate; all but the first false if the create
/// fails.
fn channel_checks(pid: ProcessId) -> [bool; 5] {
    let mut checks = [false; 5];
    let Some((create_handle, create_token)) = live_token(pid, &Capability::ChannelCreate) else {
        return checks;
    };

    let (x0, x1) = svc(Syscall::ChannelCreate, &[]);
    checks[0] = x0 >= MIN_VALID_ID;
    if !checks[0] {
        return checks;
    }
    // x0 is at least 256 and came from a zero-extended u32.
    let channel = ChannelId::from_raw(x0 as u32);

    checks[1] = minted_child(pid, x1, Capability::ChannelAccess(channel), create_token);
    checks[2] = {
        let mut table = CHANNEL_TABLE.lock();
        channel_mut(&mut table, channel)
            .is_ok_and(|ch| ch.creator == Some(pid) && ch.creation_cap == Some(create_token))
    };
    checks[3] = attenuate_denied(create_handle, ATTENUATE_CHANNEL_ACCESS, x0);
    checks[4] = channel_destroy(channel).is_ok();
    checks
}

/// The SharedMemoryCreate checks, 5-10 of `create_abi_test`, under a
/// temporary grant T that is revoked before return (check 10). All false if
/// the grant fails; only check 10 can pass if the create fails.
fn shm_checks(pid: ProcessId) -> [bool; 6] {
    let mut checks = [false; 6];
    let granted = crate::cap::grant_to_process(pid, Capability::SharedMemoryCreate, false)
        .ok()
        .and_then(|handle| Some((handle, token_id(pid, handle)?)));
    let Some((grant_handle, grant_token)) = granted else {
        return checks;
    };

    let (x0, x1) = svc(Syscall::SharedMemoryCreate, &[4096, READ_WRITE]);
    checks[0] = x0 >= MIN_VALID_ID;
    if checks[0] {
        // x0 is at least 256 and came from a zero-extended u32.
        let region = SharedMemoryId::from_raw(x0 as u32);
        checks[1] = minted_child(pid, x1, Capability::SharedMemoryAccess(region), grant_token);
        checks[2] = {
            let mut table = shmem::SHARED_REGION_TABLE.lock();
            shmem::region_mut(&mut table, region).is_ok_and(|r| r.creation_cap == grant_token)
        };
        checks[3] = attenuate_denied(grant_handle, ATTENUATE_SHM_ACCESS, x0);
        // The minted token authorises the map; the unmap of the only mapping
        // frees the region.
        let flags = VmFlags::READ.union(VmFlags::WRITE);
        checks[4] = shmem::shared_memory_map(pid, region, flags).is_ok()
            && shmem::shared_memory_unmap(pid, region).is_ok();
    }
    checks[5] = revoke_in_process(pid, grant_token).is_ok();
    checks
}

/// Issue syscall `nr` with `args` in x0.. through `syscall_dispatch` and
/// return x0 as the signed result and x1 as it was left.
fn svc(nr: Syscall, args: &[u64]) -> (i64, u64) {
    let mut tf = TrapFrame {
        x: [0; 31],
        sp_el0: 0,
        elr_el1: 0,
        spsr_el1: 0,
    };
    tf.x[8] = nr as u64;
    tf.x[..args.len()].copy_from_slice(args);
    crate::syscall::syscall_dispatch(&mut tf);
    (tf.x[0] as i64, tf.x[1])
}

/// The handle and id of the first live, unexpired token for exactly `cap` in
/// `pid`'s capability table.
fn live_token(pid: ProcessId, cap: &Capability) -> Option<(CapabilityHandle, CapabilityTokenId)> {
    let now = crate::arch::aarch64::timer::TICK_COUNT.load(core::sync::atomic::Ordering::Relaxed);
    let table = PROCESS_TABLE.lock();
    let proc = process_ref(&table, pid).ok()?;
    proc.cap_table
        .tokens()
        .iter()
        .enumerate()
        .find_map(|(i, slot)| {
            slot.as_ref()
                .filter(|t| !t.revoked && !t.is_expired(now) && t.capability == *cap)
                // The table has MAX_CAPS_PER_PROCESS (256) slots, so `i` fits.
                .map(|t| (CapabilityHandle(i as u32), t.id))
        })
}

/// Whether the raw handle `x1` names a live token in `pid`'s table for
/// exactly `cap`, minted as a child of `parent`.
fn minted_child(pid: ProcessId, x1: u64, cap: Capability, parent: CapabilityTokenId) -> bool {
    let Ok(raw) = u32::try_from(x1) else {
        return false;
    };
    let token: Option<CapabilityToken> = {
        let table = PROCESS_TABLE.lock();
        process_ref(&table, pid)
            .ok()
            .and_then(|proc| proc.cap_table.get(CapabilityHandle(raw)).cloned())
    };
    token.is_some_and(|t| t.capability == cap && t.parent_token == Some(parent))
}

/// Whether CapabilityAttenuate from the Create token at `handle` to
/// `new_cap_type` for the object `id` returns EPERM: a Create token never
/// attenuates to an Access one.
fn attenuate_denied(handle: CapabilityHandle, new_cap_type: u64, id: i64) -> bool {
    let (x0, _) = svc(
        Syscall::CapabilityAttenuate,
        &[u64::from(handle.0), new_cap_type, 0, id as u64],
    );
    x0 == IpcError::Eperm as i64
}
