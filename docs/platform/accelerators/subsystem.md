# AIOS Compute Subsystem

Part of: [accelerators.md](../accelerators.md) — Platform Accelerator Drivers
**Related:** [drivers.md](./drivers.md) — AcceleratorDriver trait, [subsystem-framework.md](../subsystem-framework.md) — Subsystem framework, [compute/security.md](../../kernel/compute/security.md) — ComputeAccess capability

-----

## 10. Compute Subsystem

The compute subsystem follows the AIOS subsystem framework ([subsystem-framework.md](../subsystem-framework.md) §3-§4). It implements the `Subsystem` trait with compute-specific session management, capability gating, and audit logging. Like audio, camera, and networking, the compute subsystem provides a unified service interface that abstracts over multiple device drivers.

### 10.1 Subsystem Trait Implementation

```rust
/// Compute subsystem implementing the universal Subsystem trait.
///
/// The compute subsystem manages all accelerator devices and provides
/// session-based access for agents. It sits between AIRS (which makes
/// placement decisions) and the accelerator drivers (which program
/// hardware).
pub struct ComputeSubsystem {
    /// Subsystem identity.
    id: SubsystemId,
    /// Active compute sessions.
    sessions: BTreeMap<SessionId, ComputeSession>,
    /// Registered accelerator drivers.
    drivers: Vec<Box<dyn AcceleratorDriver>>,
    /// Reference to the kernel's ComputeRegistry.
    registry: ComputeRegistryHandle,
    /// Audit logger for compute operations.
    audit: AuditLogger,
    /// Session counter for unique IDs.
    next_session_id: u64,
}

impl Subsystem for ComputeSubsystem {
    const ID: SubsystemId = SubsystemId::Compute;

    type Capability = ComputeCapability;
    type Session = ComputeSession;
    type DeviceInfo = ComputeDeviceInfo;

    fn create_session(
        &mut self,
        agent_id: AgentId,
        capability: &Self::Capability,
    ) -> Result<SessionId, SubsystemError> {
        // Validate capability token
        self.validate_capability(agent_id, capability)?;

        // Create session with compute-specific state
        let session_id = SessionId(self.next_session_id);
        self.next_session_id += 1;

        let session = ComputeSession {
            id: session_id,
            agent_id,
            capability: capability.clone(),
            state: SessionState::Active,
            device_bindings: Vec::new(),
            buffer_pool: SessionBufferPool::new(capability),
            created_at: Timestamp::now(),
            last_activity: Timestamp::now(),
            stats: SessionStats::default(),
        };

        self.sessions.insert(session_id, session);

        // Audit log
        self.audit.log(AuditEvent::ComputeSessionCreated {
            agent_id,
            session_id,
            capability_level: capability.access_level(),
            timestamp: Timestamp::now(),
        });

        Ok(session_id)
    }

    fn destroy_session(
        &mut self,
        session_id: SessionId,
    ) -> Result<(), SubsystemError> {
        let session = self.sessions.remove(&session_id)
            .ok_or(SubsystemError::SessionNotFound)?;

        // Clean up device bindings
        for binding in &session.device_bindings {
            if let Some(driver) = self.find_driver(binding.device_id) {
                let _ = driver.unmap_compute_buffer(&binding.buffer);
            }
        }

        // Free session buffer pool
        session.buffer_pool.free_all();

        // Audit log
        self.audit.log(AuditEvent::ComputeSessionDestroyed {
            agent_id: session.agent_id,
            session_id,
            duration: Timestamp::now() - session.created_at,
            total_compute_time: session.stats.total_compute_time,
            total_submissions: session.stats.submissions,
            timestamp: Timestamp::now(),
        });

        Ok(())
    }

    fn list_devices(&self) -> Vec<Self::DeviceInfo> {
        self.registry.list_devices().iter().map(|entry| {
            ComputeDeviceInfo {
                device_id: entry.device_id,
                class: entry.class,
                capabilities: entry.capabilities.clone(),
                utilization: entry.utilization,
                thermal_state: entry.thermal_state,
                available: entry.available(),
            }
        }).collect()
    }
}
```

### 10.2 Compute Session

A compute session represents an agent's ongoing interaction with the compute subsystem. Sessions track state, manage buffer allocations, and enforce per-session limits:

```rust
/// An active compute session for an agent.
pub struct ComputeSession {
    /// Unique session identifier.
    pub id: SessionId,
    /// The agent that owns this session.
    pub agent_id: AgentId,
    /// The capability that authorized this session.
    pub capability: ComputeCapability,
    /// Session lifecycle state.
    pub state: SessionState,
    /// Active device bindings (buffers mapped to specific devices).
    pub device_bindings: Vec<DeviceBinding>,
    /// Per-session buffer pool (subset of system buffer pool).
    pub buffer_pool: SessionBufferPool,
    /// Session creation timestamp.
    pub created_at: Timestamp,
    /// Last activity timestamp (for idle timeout).
    pub last_activity: Timestamp,
    /// Cumulative statistics for this session.
    pub stats: SessionStats,
}

/// Session lifecycle states.
pub enum SessionState {
    /// Session is active and can accept compute requests.
    Active,
    /// Session is suspended (agent backgrounded). Device bindings
    /// remain but new submissions are rejected.
    Suspended,
    /// Session is being torn down. All pending compute must complete
    /// before buffers are freed.
    Draining,
    /// Session is closed. All resources freed.
    Closed,
}

/// Statistics tracked per compute session.
#[derive(Default)]
pub struct SessionStats {
    /// Total number of compute submissions.
    pub submissions: u64,
    /// Total compute time consumed (device-reported).
    pub total_compute_time: Duration,
    /// Total bytes transferred to/from accelerators.
    pub total_bytes_transferred: u64,
    /// Number of submissions that failed or timed out.
    pub errors: u32,
    /// Peak memory usage (bytes).
    pub peak_memory_bytes: usize,
}
```

### 10.3 Compute Capability Gate

The compute subsystem enforces capability-gated access following the subsystem framework pattern ([subsystem-framework.md](../subsystem-framework.md) §5):

```rust
/// Compute-specific capability variants for the subsystem.
pub enum ComputeCapability {
    /// Access to a specific compute device.
    DeviceAccess {
        device_id: ComputeDeviceId,
        max_memory_bytes: usize,
    },
    /// Access to any device of a given class.
    ClassAccess {
        class: ComputeClass,
        max_memory_bytes: usize,
    },
    /// System-level access (AIRS, security agents).
    SystemAccess,
}

impl ComputeCapability {
    /// Check whether this capability authorizes access to a device.
    pub fn authorizes_device(
        &self,
        device_id: &ComputeDeviceId,
    ) -> bool {
        match self {
            ComputeCapability::DeviceAccess { device_id: id, .. } => {
                id == device_id
            }
            ComputeCapability::ClassAccess { class, .. } => {
                device_id.class == *class
            }
            ComputeCapability::SystemAccess => true,
        }
    }

    /// Maximum memory allocation allowed by this capability.
    pub fn max_memory(&self) -> usize {
        match self {
            ComputeCapability::DeviceAccess { max_memory_bytes, .. } => {
                *max_memory_bytes
            }
            ComputeCapability::ClassAccess { max_memory_bytes, .. } => {
                *max_memory_bytes
            }
            ComputeCapability::SystemAccess => usize::MAX,
        }
    }

    /// Access level for audit logging.
    pub fn access_level(&self) -> &'static str {
        match self {
            ComputeCapability::DeviceAccess { .. } => "device",
            ComputeCapability::ClassAccess { .. } => "class",
            ComputeCapability::SystemAccess => "system",
        }
    }
}
```

### 10.4 Session Conflict Resolution

When multiple agents request exclusive access to the same compute device, the subsystem resolves conflicts based on priority and preemption rules:

```text
Conflict Resolution Policy:

Priority Order:
  1. System agents (AIRS, security monitor) — always admitted
  2. Interactive sessions (user-facing inference) — preempt background
  3. Background sessions (batch processing) — queued

Preemption Rules:
  - System never preempted
  - Interactive preempts background only (never other interactive)
  - Background never preempts anything

  When preemption occurs:
  1. Background session state = Suspended
  2. Current compute job runs to completion (no mid-job preemption)
  3. Device bindings preserved (buffers stay mapped)
  4. New session gets device access
  5. When new session finishes, suspended session resumes

Queue Management:
  - Max queue depth: 8 sessions per device
  - Queue timeout: 30 seconds (configurable)
  - If timeout expires: session receives ComputeError::Timeout
```

-----

## 11. POSIX Bridge

The compute subsystem provides POSIX-compatible device nodes for Linux binary compatibility ([posix.md](../posix.md) §9). These device nodes expose accelerator access through the standard file descriptor + ioctl pattern expected by Linux GPU compute frameworks.

### 11.1 Device Nodes

```text
/dev/compute/                   Compute subsystem root
├── gpu0                        First GPU device
├── gpu1                        Second GPU (if present)
├── npu0                        First NPU device
├── dsp0                        First DSP (if present)
├── cpu                         CPU compute (always present)
└── ctl                         Control device (capabilities, sessions)
```

Each device node supports the standard POSIX operations:

```text
Operation       Mapping                              Notes
─────────       ───────                              ─────
open()          Creates compute session               Requires ComputeAccess cap
close()         Destroys compute session              Frees all session resources
ioctl()         Compute-specific operations            See ioctl table below
mmap()          Maps compute buffer into agent VA      Returns buffer virtual addr
read()          Reads completion events                Blocking or O_NONBLOCK
write()         Submits compute commands               Raw command buffer
poll()/epoll()  Waits for completion or device events  POLLIN = completion ready
```

### 11.2 ioctl Interface

```rust
/// Compute subsystem ioctl numbers.
/// Follow Linux DRM ioctl numbering convention for compatibility.
pub const COMPUTE_IOCTL_BASE: u32 = 0xC0;

/// Query device capabilities.
/// Input: ComputeDeviceId
/// Output: ComputeCapabilityDescriptor
pub const COMPUTE_IOCTL_GET_CAPS: u32 = COMPUTE_IOCTL_BASE + 0x01;

/// Allocate a compute buffer.
/// Input: size, flags (ComputeBufferFlags)
/// Output: buffer_id, mmap offset
pub const COMPUTE_IOCTL_ALLOC_BUFFER: u32 = COMPUTE_IOCTL_BASE + 0x02;

/// Free a compute buffer.
/// Input: buffer_id
pub const COMPUTE_IOCTL_FREE_BUFFER: u32 = COMPUTE_IOCTL_BASE + 0x03;

/// Submit compute work.
/// Input: command buffer pointer, buffer references
/// Output: submission_id
pub const COMPUTE_IOCTL_SUBMIT: u32 = COMPUTE_IOCTL_BASE + 0x04;

/// Wait for compute completion.
/// Input: submission_id, timeout_ns
/// Output: completion status
pub const COMPUTE_IOCTL_WAIT: u32 = COMPUTE_IOCTL_BASE + 0x05;

/// Query session statistics.
/// Output: SessionStats
pub const COMPUTE_IOCTL_GET_STATS: u32 = COMPUTE_IOCTL_BASE + 0x06;

/// Set session priority.
/// Input: priority level (interactive, background)
pub const COMPUTE_IOCTL_SET_PRIORITY: u32 = COMPUTE_IOCTL_BASE + 0x07;

/// Load a pre-compiled model (NPU only).
/// Input: model data pointer, size, format
/// Output: model_handle
pub const COMPUTE_IOCTL_LOAD_MODEL: u32 = COMPUTE_IOCTL_BASE + 0x08;

/// Unload a model (NPU only).
/// Input: model_handle
pub const COMPUTE_IOCTL_UNLOAD_MODEL: u32 = COMPUTE_IOCTL_BASE + 0x09;
```

### 11.3 Linux Compatibility Layer

For Linux binaries using GPU compute frameworks (OpenCL, Vulkan Compute, CUDA-compatible), the POSIX bridge translates Linux-specific ioctls to AIOS compute subsystem operations:

```text
Linux Framework      Linux ioctl              AIOS Translation
───────────────      ───────────              ────────────────
OpenCL               /dev/dri/renderD*        /dev/compute/gpu* + translate
Vulkan Compute       /dev/dri/renderD*        /dev/compute/gpu* + translate
ONNX Runtime         Custom device API        /dev/compute/npu* via ioctl
TensorFlow Lite      Custom device API        /dev/compute/npu* via ioctl

Translation Strategy:
  1. Linux binary opens /dev/dri/renderD128
  2. POSIX bridge intercepts → opens /dev/compute/gpu0
  3. Linux DRM ioctls → COMPUTE_IOCTL_* translations
  4. GEM buffer alloc → COMPUTE_IOCTL_ALLOC_BUFFER
  5. DRM submit → COMPUTE_IOCTL_SUBMIT
  6. DRM wait → COMPUTE_IOCTL_WAIT

Not all DRM ioctls are translatable. Unsupported ioctls
return ENOTTY. The subset needed for compute (not display)
is small: buffer management + command submission.
```

### 11.4 Audit Integration

Every POSIX operation on compute device nodes generates an audit event, consistent with the subsystem framework's audit pattern ([subsystem-framework.md](../subsystem-framework.md) §7):

```text
Audit Events from POSIX Bridge:

Event                   Triggered By           Logged Fields
─────                   ────────────           ─────────────
ComputeDeviceOpened     open(/dev/compute/*)   agent_id, device, timestamp
ComputeBufferAllocated  ioctl(ALLOC_BUFFER)    agent_id, size, device
ComputeWorkSubmitted    ioctl(SUBMIT)          agent_id, device, est_time
ComputeWorkCompleted    completion callback    agent_id, device, actual_time
ComputeModelLoaded      ioctl(LOAD_MODEL)      agent_id, model_hash, size
ComputeDeviceClosed     close(fd)              agent_id, session_duration

User query via Inspector:
  "Which agents used the GPU in the last hour?"
  → SELECT * FROM audit WHERE subsystem='compute'
    AND device LIKE 'gpu%' AND timestamp > now() - 1h
```
