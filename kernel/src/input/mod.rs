//! Input subsystem — translates raw VirtIO-input events into typed InputEvents.
//!
//! Tracks modifier state, converts absolute tablet coordinates to display
//! coordinates, and accumulates pointer events between SYN_REPORT boundaries
//! for correct evdev atomic grouping.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use shared::collections::FixedQueue;
use shared::input::*;
use spin::Mutex;

use crate::drivers::virtio_input;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Typed input event queue. Consumed by the compositor (M24+) or demo thread.
static INPUT_QUEUE: Mutex<FixedQueue<InputEvent, 256>> = Mutex::new(FixedQueue::new());

/// Current modifier key state (SHIFT | CTRL | ALT | SUPER bitmask).
static MODIFIER_STATE: AtomicU8 = AtomicU8::new(0);

/// Flag set by timer tick every 16ms to trigger deferred polling.
static INPUT_POLL_DUE: AtomicBool = AtomicBool::new(false);

/// Pending pointer state — accumulated between SYN_REPORT boundaries.
static PENDING_POINTER: Mutex<PendingPointer> = Mutex::new(PendingPointer::new());

/// Accumulated pointer state flushed on SYN_REPORT.
struct PendingPointer {
    x: u32,
    y: u32,
    button: Option<MouseButton>,
    button_state: Option<ButtonState>,
    dirty: bool,
}

impl PendingPointer {
    const fn new() -> Self {
        Self {
            x: 0,
            y: 0,
            button: None,
            button_state: None,
            dirty: false,
        }
    }
}

/// Display dimensions for coordinate conversion (cached from GPU).
static DISPLAY_WIDTH: AtomicU32 = AtomicU32::new(1280);
static DISPLAY_HEIGHT: AtomicU32 = AtomicU32::new(800);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the input subsystem.
///
/// Caches display dimensions from the GPU driver for coordinate conversion.
/// Creates the input polling thread.
pub fn init(device_count: usize) {
    // Cache display dimensions from GPU (if available).
    if let Some(info) = crate::drivers::virtio_gpu::display_info() {
        DISPLAY_WIDTH.store(info.width, Ordering::Relaxed);
        DISPLAY_HEIGHT.store(info.height, Ordering::Relaxed);
    }

    crate::kinfo!(
        Input,
        "Input subsystem initialized: {} devices, display {}x{}",
        device_count,
        DISPLAY_WIDTH.load(Ordering::Relaxed),
        DISPLAY_HEIGHT.load(Ordering::Relaxed)
    );

    // Create the input polling thread.
    create_input_thread();
}

/// Signal that input polling is due (called from timer_tick_handler).
///
/// Lightweight — just an atomic store. Must not acquire any Mutex.
pub fn set_poll_due() {
    INPUT_POLL_DUE.store(true, Ordering::Release);
}

/// Poll devices if the timer flag is set, then process raw events.
///
/// Called from the input thread (non-IRQ context). Acquires INPUT_DEVICES
/// briefly to collect raw events, then releases it before processing
/// (avoids holding INPUT_DEVICES while acquiring PENDING_POINTER/INPUT_QUEUE).
pub fn poll_if_due() {
    if !INPUT_POLL_DUE.swap(false, Ordering::Acquire) {
        return;
    }

    // Collect raw events while holding INPUT_DEVICES lock.
    let mut raw_buf = [(
        InputDeviceId(0),
        VirtioInputEvent {
            event_type: 0,
            code: 0,
            value: 0,
        },
    ); 64];
    let count = virtio_input::poll_all(&mut raw_buf);

    // Process events WITHOUT holding INPUT_DEVICES lock.
    for &(_device_id, event) in raw_buf.iter().take(count) {
        process_raw_event(event);
    }
}

/// Pop the next typed event from the input queue.
pub fn pop_event() -> Option<InputEvent> {
    INPUT_QUEUE.lock().pop_front()
}

// ---------------------------------------------------------------------------
// Event translation
// ---------------------------------------------------------------------------

/// Translate a raw VirtioInputEvent into typed InputEvent(s).
///
/// Keyboard events (EV_KEY with code < BTN_LEFT) are pushed immediately.
/// Pointer events (EV_ABS, EV_KEY with code >= BTN_LEFT) accumulate in
/// PENDING_POINTER and are flushed as a single InputEvent::Pointer on
/// EV_SYN/SYN_REPORT.
fn process_raw_event(raw: VirtioInputEvent) {
    match raw.event_type {
        EV_KEY => {
            if raw.code < BTN_LEFT {
                // Keyboard event.
                process_key_event(raw.code, raw.value);
            } else {
                // Pointer button event.
                process_button_event(raw.code, raw.value);
            }
        }
        EV_ABS => {
            process_abs_event(raw.code, raw.value);
        }
        EV_SYN if raw.code == SYN_REPORT => {
            flush_pending_pointer();
        }
        _ => {
            // Ignore other event types (EV_REL, EV_MSC, etc.)
        }
    }
}

/// Process a keyboard key event.
fn process_key_event(code: u16, value: u32) {
    let key = KeyCode::from_evdev(code);

    // Update modifier state.
    update_modifiers(code, value);

    let state = match KeyState::from_value(value) {
        Some(s) => s,
        None => return, // Invalid value — skip.
    };

    let modifiers = Modifiers(MODIFIER_STATE.load(Ordering::Relaxed));

    let event = InputEvent::Keyboard {
        key,
        state,
        modifiers,
    };

    let mut queue = INPUT_QUEUE.lock();
    if !queue.push_back(event) {
        crate::kwarn!(Input, "input queue full, dropping keyboard event");
    }
}

/// Process a pointer button event (accumulate in pending state).
fn process_button_event(code: u16, value: u32) {
    let button = match code {
        BTN_LEFT => MouseButton::Left,
        BTN_RIGHT => MouseButton::Right,
        BTN_MIDDLE => MouseButton::Middle,
        _ => return,
    };

    let state = if value != 0 {
        ButtonState::Pressed
    } else {
        ButtonState::Released
    };

    let mut pending = PENDING_POINTER.lock();
    pending.button = Some(button);
    pending.button_state = Some(state);
    pending.dirty = true;
}

/// Process an absolute axis event (accumulate in pending state).
fn process_abs_event(code: u16, value: u32) {
    let mut pending = PENDING_POINTER.lock();

    match code {
        ABS_X => {
            let max_x = get_abs_max_x();
            let width = DISPLAY_WIDTH.load(Ordering::Relaxed);
            pending.x = abs_to_display(value, max_x, width);
            pending.dirty = true;
        }
        ABS_Y => {
            let max_y = get_abs_max_y();
            let height = DISPLAY_HEIGHT.load(Ordering::Relaxed);
            pending.y = abs_to_display(value, max_y, height);
            pending.dirty = true;
        }
        _ => {} // Ignore other axes.
    }
}

/// Flush pending pointer state as an InputEvent::Pointer on SYN_REPORT.
fn flush_pending_pointer() {
    let mut pending = PENDING_POINTER.lock();
    if !pending.dirty {
        return;
    }

    let event = InputEvent::Pointer {
        x: pending.x,
        y: pending.y,
        button: pending.button,
        state: pending.button_state,
    };

    // Clear button state (one-shot per report), keep position (persists).
    pending.button = None;
    pending.button_state = None;
    pending.dirty = false;

    // Push to queue (drop pending lock first to avoid holding two locks).
    drop(pending);

    let mut queue = INPUT_QUEUE.lock();
    if !queue.push_back(event) {
        crate::kwarn!(Input, "input queue full, dropping pointer event");
    }
}

// ---------------------------------------------------------------------------
// Modifier tracking
// ---------------------------------------------------------------------------

/// Update modifier state based on key press/release.
fn update_modifiers(code: u16, value: u32) {
    let flag = match code {
        KEY_LEFTSHIFT | KEY_RIGHTSHIFT => Modifiers::SHIFT,
        KEY_LEFTCTRL | KEY_RIGHTCTRL => Modifiers::CTRL,
        KEY_LEFTALT | KEY_RIGHTALT => Modifiers::ALT,
        KEY_LEFTMETA | KEY_RIGHTMETA => Modifiers::SUPER,
        _ => return,
    };

    if value != 0 {
        // Pressed or repeat — set flag.
        MODIFIER_STATE.fetch_or(flag, Ordering::Relaxed);
    } else {
        // Released — clear flag.
        MODIFIER_STATE.fetch_and(!flag, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Abs axis info helpers
// ---------------------------------------------------------------------------

/// Get the maximum X axis value from the first tablet device.
fn get_abs_max_x() -> u32 {
    for i in 0..virtio_input::MAX_INPUT_DEVICES {
        if let Some((max_x, _)) = virtio_input::get_abs_info(i) {
            return max_x;
        }
    }
    32767 // Default (QEMU virtio-tablet).
}

/// Get the maximum Y axis value from the first tablet device.
fn get_abs_max_y() -> u32 {
    for i in 0..virtio_input::MAX_INPUT_DEVICES {
        if let Some((_, max_y)) = virtio_input::get_abs_info(i) {
            return max_y;
        }
    }
    32767 // Default.
}

// ---------------------------------------------------------------------------
// Input polling thread
// ---------------------------------------------------------------------------

/// Create the input polling/demo thread.
fn create_input_thread() {
    use crate::sched;
    use crate::task::Thread;
    use shared::sched::{CpuSet, SchedulerClass, ThreadId};

    let stack_phys = sched::alloc_kernel_stack();
    let stack_virt_top = sched::phys_to_virt(stack_phys) + sched::STACK_SIZE;

    let mut thread = Thread::new_kernel(
        ThreadId(0xA00), // Debug label only — not the THREAD_TABLE index.
        b"input-poll\0\0\0\0\0\0",
        input_poll_entry as *const () as usize,
        stack_phys,
    );
    thread.sched.class = SchedulerClass::Interactive;
    thread.sched.effective_class = SchedulerClass::Interactive;
    thread.sched.affinity = CpuSet::all();
    thread.context.sp = stack_virt_top as u64;

    let idx = sched::allocate_thread(thread).expect("thread table full for input-poll");
    sched::enqueue_on_cpu(0, ThreadId(idx as u32), SchedulerClass::Interactive);

    crate::kinfo!(Input, "Input polling thread created (tid={})", idx);
}

/// Input polling thread entry point.
///
/// Polls input devices every ~16ms, translates raw events, and logs
/// typed events to UART for the M23 demo.
fn input_poll_entry() -> ! {
    // Unmask IRQs — enter_scheduler left them masked when it dispatched us.
    // SAFETY: DAIFClr #0x2 clears the IRQ mask bit. Safe at EL1.
    unsafe { core::arch::asm!("msr DAIFClr, #0x2") };

    loop {
        poll_if_due();

        // Drain and log events (M23 demo output).
        while let Some(event) = pop_event() {
            match event {
                InputEvent::Keyboard {
                    key,
                    state,
                    modifiers,
                } => {
                    crate::kinfo!(Input, "Key: {:?} {:?} mod={:#04x}", key, state, modifiers.0);
                }
                InputEvent::Pointer {
                    x,
                    y,
                    button,
                    state,
                } => {
                    if let (Some(btn), Some(st)) = (button, state) {
                        crate::kinfo!(Input, "Pointer: x={} y={} {:?} {:?}", x, y, btn, st);
                    } else {
                        crate::kinfo!(Input, "Pointer: x={} y={}", x, y);
                    }
                }
            }
        }

        // Sleep ~16ms (16 ticks at 1 kHz).
        crate::ipc::sleep_ticks(16);
    }
}
