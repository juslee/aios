# Architecture Document Map

Topic-to-document index for AIOS architecture docs. Loaded on demand (not in `CLAUDE.md`).

| Topic | Document | Key Sections |
| --- | --- | --- |
| System overview & vision | `docs/project/overview.md` | §1 Vision, §2 Architecture |
| Development plan & phases | `docs/project/development-plan.md` | §3 Dependencies, §5 Gates (incl. Gate 1 retro), §8 Phase table, §8.1 Actual progress |
| Full architecture | `docs/project/architecture.md` | All |
| Language ecosystem (hub) | `docs/project/language-ecosystem.md` | §1 Overview, Document Map, Impl Order |
| Language runtimes | `docs/project/language-ecosystem/runtimes.md` | §2 Rust, §3 Python, §4 TypeScript (QuickJS-ng), §5 WASM (wasmtime + WAMR) |
| Language integration & build plan | `docs/project/language-ecosystem/integration.md` | §6 Dependency chain, §7 Build plan, §8 Key decisions, RuntimeAdapter trait |
| Language operations & security | `docs/project/language-ecosystem/operations.md` | §9 Interop (WIT/Component Model), §10 Observability, §11 Supply chain, §12 Resource isolation |
| Language AI optimization | `docs/project/language-ecosystem/ai.md` | §13 AIRS Runtime Advisor/scheduling/allocation/GC/anomaly, §14 Future directions |
| Boot sequence (hub) | `docs/kernel/boot.md` | §1 Overview, Document Map, Future Directions |
| Firmware handoff (BootInfo, ESP, EL model) | `docs/kernel/boot/firmware.md` | §2.1–§2.6 |
| Kernel early boot (boot.S, kernel_main) | `docs/kernel/boot/kernel.md` | §3.1–§3.6 |
| Service Manager boot phases | `docs/kernel/boot/services.md` | §4–§5 |
| Boot performance & framebuffer | `docs/kernel/boot/performance.md` | §6–§7 |
| Panic handler, recovery, initramfs | `docs/kernel/boot/recovery.md` | §8–§10 |
| Shutdown, implementation order, principles | `docs/kernel/boot/lifecycle.md` | §11, §12, §23, §24 |
| Boot test strategy | `docs/kernel/boot/testing.md` | §13–§14 |
| Suspend/resume, semantic state | `docs/kernel/boot/suspend.md` | §15 |
| Boot intelligence, on-demand services | `docs/kernel/boot/intelligence.md` | §16–§18 |
| Boot accessibility, first boot | `docs/kernel/boot/accessibility.md` | §19–§21 |
| Research kernel innovations | `docs/kernel/boot/research.md` | §22.1–§22.19 |
| Device model & driver framework (hub) | `docs/kernel/device-model.md` | §1 Core Insight, §2 Architecture, §17 Impl Order, §18 Design Principles, Document Map |
| Device representation & registry | `docs/kernel/device-model/representation.md` | §3 HardwareDescriptor/DeviceId/DeviceNode, §4 DeviceRegistry |
| Bus abstraction & driver model | `docs/kernel/device-model/discovery.md` | §5 Bus trait (Platform/VirtIO/USB/PCI), §6 Driver trait/matching/binding |
| Device lifecycle & driver isolation | `docs/kernel/device-model/lifecycle.md` | §7 State machine, §8 DriverGrant/interrupt forwarding/DMA sharing, §9 Crash recovery |
| VirtIO MMIO transport | `docs/kernel/device-model/virtio.md` | §10 Virtqueue internals, descriptor tables, scatter-gather |
| DMA engine & subsystem patterns | `docs/kernel/device-model/dma.md` | §11 Buffer lifecycle/IOMMU/cache coherency, §12 Per-subsystem patterns |
| Device security & hot-swap | `docs/kernel/device-model/security.md` | §13 Capability-gated MMIO/IRQ/DMA, §14 Live driver update |
| Device testing & AI intelligence | `docs/kernel/device-model/intelligence.md` | §15 Testing/verification, §16 AI-native intelligence, §19 Future directions |
| HAL & Platform trait | `docs/kernel/hal.md` | §2-3 |
| PL011 UART driver | `docs/kernel/hal.md` | §4.3 |
| GICv3 interrupt controller | `docs/kernel/hal.md` | §4.1 |
| ARM Generic Timer | `docs/kernel/hal.md` | §4.2 |
| Memory management (hub) | `docs/kernel/memory.md` | §1 Overview, §14 Impl order, doc map |
| Physical memory (buddy allocator) | `docs/kernel/memory/physical.md` | §2.2 BuddyAllocator, §2.3 FrameAllocator, §2.4 PagePools |
| Slab allocator & heap | `docs/kernel/memory/physical.md` | §4.1 SlabAllocator, §4.2 Kernel Heap |
| Virtual memory & page tables | `docs/kernel/memory/virtual.md` | §3.2 PageTableEntry, §3.3 KASLR, §3.4 TLB/ASID |
| Per-agent address spaces | `docs/kernel/memory/virtual.md` | §5 Per-Agent Memory, §7 Shared Memory |
| AI model memory | `docs/kernel/memory/ai.md` | §6 Model regions, PagedAttention, KV caches |
| Memory pressure & reclamation | `docs/kernel/memory/reclamation.md` | §8 Pressure/OOM, §10 Swap/zram, §12 Scaling |
| Memory hardening | `docs/kernel/memory/hardening.md` | §9 W^X/PAC/BTI/MTE, §11 Perf, §13 Future |
| IPC & syscalls | `docs/kernel/ipc.md` | All (Phase 3+) |
| Scheduler | `docs/kernel/scheduler.md` | All (Phase 3+) |
| Deadlock prevention | `docs/kernel/deadlock-prevention.md` | All (Phase 3+) |
| Kernel observability | `docs/kernel/observability.md` | All (Phase 3+) |
| Heterogeneous compute (hub) | `docs/kernel/compute.md` | §1 Core Insight, §2 Architecture, §15 Impl Order, §16 Design Principles, Document Map |
| Compute classification | `docs/kernel/compute/classification.md` | §3 ComputeDevice trait/ComputeClass/ComputeDeviceId, §4 ComputeCapabilityDescriptor |
| Compute registry | `docs/kernel/compute/registry.md` | §5 ComputeRegistry, §6 ComputeTopology |
| Compute budget | `docs/kernel/compute/budget.md` | §7 ComputeBudget, §8 ComputeQuota |
| Compute memory model | `docs/kernel/compute/memory.md` | §9 ComputeMemoryModel, §10 Zero-Copy Buffer Exchange |
| Compute security | `docs/kernel/compute/security.md` | §11 ComputeAccess capability, §12 Command Stream Isolation |
| Compute intelligence | `docs/kernel/compute/intelligence.md` | §13 Cross-Device Thermal Coupling, §14 Kernel-Internal ML, §17 Future Directions |
| Space Storage (hub) | `docs/storage/spaces.md` | §1 Core Insight, §2 Architecture, §11 Design Principles, §12 Impl Order, Document Map |
| Storage data structures | `docs/storage/spaces/data-structures.md` | §3.0–§3.4 Primitive types, Spaces, Objects, CompactObject, Relations |
| Block Engine | `docs/storage/spaces/block-engine.md` | §4.1–§4.10 On-disk layout, LSM-tree, WAL, compression, encryption, WAF |
| Version Store | `docs/storage/spaces/versioning.md` | §5.1–§5.5 Merkle DAG, snapshots, retention, branching |
| Storage encryption | `docs/storage/spaces/encryption.md` | §6.1–§6.3 Key management, nonces, encryption zones |
| Query Engine | `docs/storage/spaces/query-engine.md` | §7.1–§7.6 Query dispatch, full-text, embeddings, learned indexes |
| Space Sync | `docs/storage/spaces/sync.md` | §8.1–§8.4 Merkle exchange, conflict resolution, sync security |
| POSIX compatibility (storage) | `docs/storage/spaces/posix.md` | §9.1–§9.6 Path mapping, translation layer, fd lifecycle |
| Storage budget & pressure | `docs/storage/spaces/budget.md` | §10.1–§10.9 Device profiles, quotas, pressure, AI-driven storage |
| Flow (hub) | `docs/storage/flow.md` | §1 Overview, §2 Architecture, §13 Impl order, §14 Principles, Document Map |
| Flow data model | `docs/storage/flow/data-model.md` | §3.0–§3.4 External types, FlowEntry, transfer lifecycle, TypedContent |
| Flow transforms | `docs/storage/flow/transforms.md` | §4.1–§4.3 Transform engine, pipeline, registry, conversion graph |
| Flow history & sync | `docs/storage/flow/history.md` | §5.1–§5.3 History storage/UI/retention, §9.1–§9.2 Multi-device sync |
| Flow integration | `docs/storage/flow/integration.md` | §6 Compositor, §7 Subsystem channels, §8 Cross-agent, §10 POSIX bridge |
| Flow security | `docs/storage/flow/security.md` | §11.1–§11.3 Capability enforcement, content screening, rate limiting |
| Flow SDK | `docs/storage/flow/sdk.md` | §12.1–§12.3 Rust/Python/TypeScript APIs, PWA web API |
| Flow extensions | `docs/storage/flow/extensions.md` | §15.1–§15.8 Near-term, §16.1–§16.11 Future directions |
| Compositor (hub) | `docs/platform/compositor.md` | §1 Core Insight, §2 Architecture, §15 Design Principles, §16 Impl Order, Document Map |
| Compositor protocol | `docs/platform/compositor/protocol.md` | §3.1–§3.4 Surface lifecycle, shared buffers, fences, damage; §4.1–§4.4 Semantic hints, content types, hint-driven behavior |
| Compositor rendering | `docs/platform/compositor/rendering.md` | §5.1–§5.5 Scene graph, frame composition, direct scanout, frame scheduling, animation; §6.1–§6.4 Layout, multi-monitor, HDR |
| Compositor input | `docs/platform/compositor/input.md` | §7.1–§7.6 Input pipeline, focus, hotkeys, gestures, gamepad/touch, secure input |
| Compositor GPU | `docs/platform/compositor/gpu.md` | §8.1–§8.5 wgpu, VirtIO-GPU, VC4/V3D, GPU memory, shaders; §9.1–§9.5 Wayland, XWayland, DRM/KMS, security context |
| Compositor security | `docs/platform/compositor/security.md` | §10.1–§10.5 Capability-gated surfaces, GPU isolation, capture, clipboard, trust levels; §11.1–§11.5 Accessibility |
| Compositor AI-native | `docs/platform/compositor/ai-native.md` | §12.1–§12.8 AIRS-dependent compositing; §13.1–§13.8 Kernel-internal ML; §14 Future directions |
| GPU & Display (hub) | `docs/platform/gpu.md` | §1 Core Insight, §2 Architecture, §19 Impl Order, §20 Design Principles, Document Map |
| GPU drivers | `docs/platform/gpu/drivers.md` | §3 VirtIO-GPU driver, §4 Platform-specific drivers (VC4/V3D, AGX), §5 Software renderer |
| GPU display | `docs/platform/gpu/display.md` | §6 Display controller, §7 Framebuffer management, §8 Display pipeline |
| GPU rendering | `docs/platform/gpu/rendering.md` | §9 wgpu integration, §10 Rendering pipeline, §11 Font rendering, §12 GPU memory management |
| GPU security | `docs/platform/gpu/security.md` | §13 Capability-gated GPU access, §14 DMA protection, §15 GPU isolation |
| GPU integration | `docs/platform/gpu/integration.md` | §16 POSIX compatibility, §17 AI-native display, §18 Future directions |
| Audio subsystem (hub) | `docs/platform/audio.md` | §1 Overview, §14 Impl Order, §15 Design Principles, §16 Future Directions, §17 AI-Native Audio, Document Map |
| Audio subsystem & sessions | `docs/platform/audio/subsystem.md` | §2 Architecture, §3.1–§3.4 Sessions/capabilities/routing/conflict + AIRS integration hooks |
| Audio mixing & capture | `docs/platform/audio/mixing.md` | §4.1–§4.6 Mixer/SRC/capture pipeline/DSP filter graph, §10.1–§10.2 Format types/negotiation |
| Audio drivers | `docs/platform/audio/drivers.md` | §5.1–§5.7 VirtIO-Sound/I2S/PWM/HDMI/Apple/USB/privacy-first hardware |
| Audio scheduling & sync | `docs/platform/audio/scheduling.md` | §6.1–§6.4 RT scheduling/latency/buffers + predictive hints, §7.1–§7.4 Timeline/sync |
| Audio integration | `docs/platform/audio/integration.md` | §8 HDMI, §9 Power, §11 Audit + visual mic indicator, §12 POSIX, §13 Boot chime |
| USB subsystem (hub) | `docs/platform/usb.md` | §1 Overview, §12 Impl Order, §13 Design Principles, §14 Future Directions, Document Map |
| USB controller architecture | `docs/platform/usb/controller.md` | §2.1–§2.7 UsbHostController trait, xHCI, DWC2, discovery, DMA, interrupts, performance |
| USB device classes | `docs/platform/usb/device-classes.md` | §3.1–§3.3 Enumeration, §4.1–§4.7 HID/storage/audio/video/network/serial/accessibility, §5 Routing |
| USB hotplug & power | `docs/platform/usb/hotplug.md` | §6.1–§6.3 Hub enumeration, §7.1–§7.4 Hotplug state machine, §8.1–§8.4 Power management |
| USB security & AI | `docs/platform/usb/security.md` | §9.1–§9.5 Threat model/caps/IOMMU/fuzzing/allowlist, §10.1–§10.4 AI-native, §11.1–§11.3 Audit |
| Networking (hub) | `docs/platform/networking.md` | §1 Core Insight, §2 Full Architecture, §7 Impl Order, §8 Tech Choices, §10 Design Principles, Document Map |
| ANM specification | `docs/platform/networking/anm.md` | §A1–§A8 ANM 5-layer model, data units, encapsulation, design principles, ANM vs OSI, failure modes, tech stack |
| Mesh Layer | `docs/platform/networking/mesh.md` | §M1–§M10 Identity Layer, Noise IK protocol, transport modes (Direct/Relay/Tunnel), peer discovery, peer table, mesh packet format, capability exchange |
| Bridge Module | `docs/platform/networking/bridge.md` | §B1–§B7 Bridge components, translation flows, bridge security (7 layers), honest limitations, protocol integration guide (WireGuard reference), POSIX socket emulation |
| NTM components | `docs/platform/networking/components.md` | §3.0–§3.6 Mesh Manager, Space Resolver, Connection Manager (mesh + bridge), Shadow Engine, Resilience Engine, Capability Gate, Bandwidth Scheduler |
| Network stacks | `docs/platform/networking/stack.md` | §4.0–§4.7 Mesh Stack overview, Bridge Stack (smoltcp), VirtIO-Net (dual-use), buffer management, zero-copy, interrupt handling, DHCP/DNS |
| Protocol engines | `docs/platform/networking/protocols.md` | §5.1–§5.5 AIOS Mesh Protocol (native), Bridge protocols (HTTP/2, QUIC, WebSocket/SSE, TLS/rustls) |
| Network security | `docs/platform/networking/security.md` | §6.0–§6.5 ANM security model (5 layers), capability gate, packet filtering (mesh vs bridge), per-agent isolation, credential vault, graduated trust |
| Networking examples | `docs/platform/networking/examples.md` | §9.0–§9.5 Mesh-first examples (space sync, cap delegation), Bridge examples (web browsing, POSIX compat, credential routing) |
| Networking future | `docs/platform/networking/future.md` | §11.1–§11.9 AI-driven networking, learned congestion, predictive prefetch, anomaly detection, mesh-specific research (onion routing, PQ crypto) |
| Input subsystem (hub) | `docs/platform/input.md` | §1 Core Insight, §2 Architecture, §7 Impl Order, §8 Tech Choices, §9 Design Principles, Document Map |
| Input devices & HID | `docs/platform/input/devices.md` | §3.1–§3.7 Device taxonomy, USB HID protocol, VirtIO-input, Bluetooth HID, accessibility devices, hotplug |
| Input event model & dispatch | `docs/platform/input/events.md` | §4.1–§4.6 Event hierarchy, pipeline stages, queuing, focus routing, hotkeys, multi-seat |
| Input gesture recognition | `docs/platform/input/gestures.md` | §5.1–§5.5 Keyboard processing, mouse/trackpad, touchscreen, gamepad, three-layer gesture architecture |
| Input system integration | `docs/platform/input/integration.md` | §6.1–§6.6 Capability system, POSIX bridge, power management, audit, compositor, UI toolkit |
| Input AI-native intelligence | `docs/platform/input/ai.md` | §10.1–§10.7 Predictive input, adaptive params, gesture learning, anomaly detection, shortcuts, accessibility ML |
| Input future directions | `docs/platform/input/future.md` | §11.1–§11.6 Spatial input, voice, neural/BCI, haptics, cross-device, formal verification |
| Wireless (hub) | `docs/platform/wireless.md` | §1 Core Insight, §2 Architecture, §11 Impl Order, §12 Tech Choices, §13 Design Principles, Document Map |
| WiFi stack | `docs/platform/wireless/wifi.md` | §3.1–§3.6 Stack layers, station management, WPA2/WPA3, frame processing, WiFi Direct, WiFi 6/6E/7 |
| Bluetooth stack | `docs/platform/wireless/bluetooth.md` | §4.1–§4.6 HCI transport, L2CAP, classic profiles (A2DP/HFP/HID), BLE GATT/HOGP, Mesh, LE Audio |
| Wireless firmware | `docs/platform/wireless/firmware.md` | §5.1–§5.5 Firmware blob strategy, loading mechanism, versioning, open firmware, regulatory domain |
| Wireless security | `docs/platform/wireless/security.md` | §6.1–§6.5 WiFi security (WPA3-SAE), Bluetooth security, capability-gated access, rogue AP detection, attack surface |
| Wireless integration | `docs/platform/wireless/integration.md` | §7.1–§7.8 Subsystem framework, USB transport, audio/input/networking integration, power, POSIX, coexistence |
| Wireless AI-native | `docs/platform/wireless/ai-native.md` | §8–§10 AIRS-dependent intelligence (18 capabilities), kernel-internal ML (14 models), future directions |
| Camera subsystem (hub) | `docs/platform/camera.md` | §1 Core Insight, §2 Architecture, §14 Impl Order, §15 Design Principles, §16 Future Directions, Document Map |
| Camera devices & discovery | `docs/platform/camera/devices.md` | §3.1–§3.4 Device taxonomy (USB/UVC, CSI/MIPI, VirtIO-Camera, depth/ToF), discovery, multi-camera topology, capabilities descriptor |
| Camera capture & ISP pipeline | `docs/platform/camera/pipeline.md` | §4.1–§4.5 Format negotiation, frame delivery, buffer management, zero-copy paths, frame timing; §5.1–§5.6 ISP stages, 3A algorithms, hardware/software ISP, still capture, RAW |
| Camera sessions | `docs/platform/camera/sessions.md` | §6.1–§6.4 Session lifecycle, SessionIntent, conflict resolution (Prompt policy), viewfinder indicator |
| Camera drivers | `docs/platform/camera/drivers.md` | §7.1–§7.5 UVC driver, CSI/MIPI driver, VirtIO-Camera, platform drivers (Pi Camera), CameraDevice trait |
| Camera privacy & security | `docs/platform/camera/security.md` | §8.1–§8.7 Hardware LED enforcement, anti-silent-capture, CameraCapability, recording consent, content screening, audit trail, physical privacy; §9.1–§9.3 Privacy indicators |
| Camera integration | `docs/platform/camera/integration.md` | §10.1–§10.6 Compositor viewfinder, Flow integration, POSIX bridge (/dev/video*, V4L2), audio sync, accessibility, input gesture bridge |
| Camera AI-native | `docs/platform/camera/ai-native.md` | §11.1–§11.5 Scene understanding, smart framing, computational photography, gesture recognition, anomaly detection; §12.1–§12.3 Kernel-internal ML; §13.1–§13.6 Future AI directions |
| Media pipeline (hub) | `docs/platform/media-pipeline.md` | §1 Core Insight, §2 Architecture, §18 Impl Order, §19 Design Principles, Document Map |
| Media codecs & containers | `docs/platform/media-pipeline/codecs.md` | §3.1–§3.5 Codec framework (MediaCodec trait, registry, HW/SW selection), §4.1–§4.4 Container engine (demuxer/muxer, MP4/WebM/MKV/MPEG-TS) |
| Media playback & sessions | `docs/platform/media-pipeline/playback.md` | §5.1–§5.6 Pipeline graph model, A/V sync, clock recovery, buffering, subtitles; §6.1–§6.4 Media sessions |
| Media streaming | `docs/platform/media-pipeline/streaming.md` | §7.1–§7.5 Protocols (HLS/DASH/MoQ/progressive), ABR; §8.1–§8.4 Network transport (jitter buffer, bandwidth, resilience) |
| Media real-time communication | `docs/platform/media-pipeline/rtc.md` | §9.1–§9.6 WebRTC stack (ICE/DTLS/RTP/SDP), simulcast/SVC; §10.1–§10.4 RTC sessions, multi-party, screen sharing |
| Media content protection | `docs/platform/media-pipeline/drm.md` | §11.1–§11.6 DRM (CDM trait, Widevine/PlayReady/FairPlay, CENC, secure decode); §12.1–§12.3 Output protection (HDCP) |
| Media integration | `docs/platform/media-pipeline/integration.md` | §13 Cross-subsystem coordination, §14 POSIX bridge (GStreamer/FFmpeg/V4L2), §15 Security/audit, §16 AI-native intelligence, §17 Thermal |
| Subsystem framework | `docs/platform/subsystem-framework.md` | §1-§4 Overview/traits, §5 Capability gate, §6 DataChannel/zero-copy, §7 Audit, §8 POSIX bridge, §9 Power, §10 Device registry, §11-§12 Hotplug/USB, §13 Audio example, §14 Subsystem summary, §15-§16 Framework benefits/Networking, §17 Error handling, §18 Testing, §19 Perf monitoring, §20 Driver model, §21 Versioning, §22 Future directions |
| POSIX compatibility | `docs/platform/posix.md` | §1-§6 Overview/arch/BSD/musl/FD/path, §7 Process+thread translation, §8 Sockets+AF_UNIX, §9 Devices, §10 Path semantics+mmap, §11-§12 Toolset/caps, §13-§14 Perf/limits, §15-§16 Linux compat/impl order, §17-§19 Principles/testing/future (Phase 27+) |
| Linux binary & Wayland compat (hub) | `docs/platform/linux-compat.md` | §1 Core Insight, §2 Architecture, §14 Impl Order, §15 Design Principles, §16 Future Directions, Document Map |
| Linux ELF loader & glibc shim | `docs/platform/linux-compat/elf-loader.md` | §3 ELF format/segments/ASLR/dynamic linker/VDSO/auxv, §4 glibc ABI shim/signals/threads |
| Linux syscall translation | `docs/platform/linux-compat/syscall-translation.md` | §5 ~200 syscall table by category, §6 Deep dives: epoll/futex/io_uring/eventfd/signalfd/timerfd |
| Linux Wayland bridge | `docs/platform/linux-compat/wayland-bridge.md` | §7 Integration architecture/buffer pipeline/frame scheduling, §8 XWayland/X11 extensions/clipboard/DnD |
| Linux sandbox & security | `docs/platform/linux-compat/sandbox.md` | §9 Threat model/capability mapping/sandbox profiles/portals/audit, §10 Comparison: Starnix/Linuxulator/WSL/gVisor |
| Linux virtual filesystems | `docs/platform/linux-compat/virtual-filesystems.md` | §11 /proc/sys/dev emulation, §12 Namespace/cgroup equivalents |
| Linux compat intelligence | `docs/platform/linux-compat/intelligence.md` | §13 AI-native improvements (syscall prediction, anomaly detection), testing/validation strategy |
| Accelerator drivers (hub) | `docs/platform/accelerators.md` | §1 Core Insight, §2 Architecture, §13 Impl Order, §14 Design Principles, Document Map |
| Accelerator driver traits | `docs/platform/accelerators/drivers.md` | §3 AcceleratorDriver trait, §4 VirtIO-GPU 3D compute, §5 VideoCore VII compute |
| Apple Neural Engine | `docs/platform/accelerators/ane.md` | §6 ANE architecture, §7 ANE driver model |
| Accelerator memory | `docs/platform/accelerators/memory.md` | §8 Platform memory management, §9 Zero-copy CPU-accelerator paths |
| Compute subsystem | `docs/platform/accelerators/subsystem.md` | §10 Compute subsystem (Subsystem trait), §11 POSIX bridge (/dev/compute/*) |
| Accelerator intelligence | `docs/platform/accelerators/intelligence.md` | §12 AIRS integration, §15 Future directions |
| Power management | `docs/platform/power-management.md` | All (Phase 32+) |
| Thermal management (hub) | `docs/platform/thermal.md` | §1 Core Insight, §14 Impl Order, §15 Design Principles, Document Map |
| Thermal zones & sensors | `docs/platform/thermal/zones.md` | §2 ThermalZone/sensors/polling/filtering, §3 Trip points/escalation/hysteresis/coupling |
| Thermal cooling & governors | `docs/platform/thermal/cooling.md` | §4 CoolingDevice trait/DVFS/fan/gating, §5 Governors (step-wise/PID/bang-bang) |
| Thermal-aware scheduling | `docs/platform/thermal/scheduling.md` | §6 ThermalState/WCET/inference/pressure, §7 Load balancing/dark silicon/core-idling |
| Thermal platform drivers | `docs/platform/thermal/platform-drivers.md` | §8 QEMU/Pi 4/Pi 5/Apple Silicon/ARM SCMI |
| Thermal integration | `docs/platform/thermal/integration.md` | §9 GPU/audio/storage/network/boot coordination, §10 POSIX/agent headroom API |
| Thermal security | `docs/platform/thermal/security.md` | §11 Capability gate/audit/safety invariants/formal verification/DoS prevention |
| Thermal intelligence | `docs/platform/thermal/intelligence.md` | §12 Kernel-internal ML (decision tree/NN/MPC/fingerprinting), §13 AIRS (DRL/GNN/multi-agent RL/anomaly) |
| BSP architecture (hub) | `docs/platform/bsp.md` | §1 Core Insight, §14 Impl Order, §15 Design Principles, Document Map |
| BSP model & porting | `docs/platform/bsp/model.md` | §2 BSP model (Platform struct, detection, DTB contract, quirks), §3 Porting checklist |
| BSP platforms | `docs/platform/bsp/platforms.md` | §4 QEMU virt, §5 Pi 4 BCM2711, §6 Pi 5 BCM2712, §7 Apple Silicon |
| BSP firmware handoff | `docs/platform/bsp/firmware.md` | §8 UEFI/U-Boot/m1n1 comparison, BootInfo adaptation |
| BSP driver mapping | `docs/platform/bsp/drivers.md` | §9 Driver mapping matrix, §10 Device tree bindings |
| BSP testing | `docs/platform/bsp/testing.md` | §11 Testing strategy, §12 Validation checklist |
| BSP intelligence | `docs/platform/bsp/intelligence.md` | §13 AI-native BSP, future ISA directions |
| Multi-device & enterprise (hub) | `docs/platform/multi-device.md` | §1 Core Insight, §2 Architecture, §11 Design Principles, §12 Impl Order, Document Map |
| Device pairing & trust | `docs/platform/multi-device/pairing.md` | §3.1–§3.5 Discovery, personal pairing (SPAKE2+), org enrollment, attestation, revocation |
| Multi-device experience | `docs/platform/multi-device/experience.md` | §4.1–§4.5 Handoff, unified clipboard, Space Mesh, intelligence continuity, display/input |
| Mobile device management | `docs/platform/multi-device/mdm.md` | §5.1–§5.5 Declarative DDM, capability-gated MDM, enrollment profiles, remote wipe, config channels |
| Fleet management | `docs/platform/multi-device/fleet.md` | §6.1–§6.5 Inventory, health monitoring, staged updates, grouping, compliance dashboard |
| Policy engine | `docs/platform/multi-device/policy.md` | §7.1–§7.6 Declarative policies, conditional access, geo-fencing, NL policies, time-based, audit trail |
| Enterprise identity | `docs/platform/multi-device/enterprise-identity.md` | §8.1–§8.4 SSO/SAML, SCIM provisioning, directory integration, multi-tenant |
| Data protection & compliance | `docs/platform/multi-device/data-protection.md` | §9.1–§9.4 DLP, content classification, provenance, encryption zones; §10.1–§10.4 SIEM, compliance frameworks, reporting, data residency |
| Multi-device intelligence | `docs/platform/multi-device/intelligence.md` | §13.1–§13.3 Kernel-internal ML (sync, anomaly, handoff); §14.1–§14.5 AIRS (GNN fleet, RL self-healing, federated learning, AI DLP, NL policy); §15 Future |
| AI Runtime AIRS (hub) | `docs/intelligence/airs.md` | §1 Core Insight, §2 Architecture, §9 Design Principles, §12 Impl Order, Document Map |
| AIRS inference engine | `docs/intelligence/airs/inference.md` | §3.1–§3.11 GGML runtime, compute scheduler, KV cache, streaming output, inference metering, session lifecycle, error handling, benchmarking, technology alternatives, AIRS-dependent intelligence, cross-references |
| AIRS model registry | `docs/intelligence/airs/model-registry.md` | §4.1–§4.6 Storage, profiles, quantization, LRU eviction, boot selection |
| AIRS intelligence services | `docs/intelligence/airs/intelligence-services.md` | §5.1–§5.9 Space Indexer, Context Engine, Attention Manager, Intent Verifier, Behavioral Monitor, Adversarial Defense, Tool Manager, Conversation Manager, Agent Capability Intelligence |
| AIRS lifecycle & data | `docs/intelligence/airs/lifecycle-and-data.md` | §6 Agent Lifecycle, §7 Data Model, §8 Key Technology Choices |
| AIRS security | `docs/intelligence/airs/security.md` | §10.1–§10.5 Security path isolation, crash containment, agent hints, kernel oversight, provenance |
| AIRS hardware scaling | `docs/intelligence/airs/scaling.md` | §11.1–§11.4 Model capability trajectory, multi-model architecture, context windows, NPU integration |
| AIRS AI-native intelligence | `docs/intelligence/airs/ai-native.md` | §13.1–§13.7 Kernel-internal ML, §14.1–§14.11 AIRS-dependent intelligence, §15 Future directions |
| Space Indexer (hub) | `docs/intelligence/space-indexer.md` | §1 Core Insight, §2 Architecture, §13 Impl Order, §14 Design Principles, §15 Future Directions, Document Map |
| Space Indexer pipeline | `docs/intelligence/space-indexer/pipeline.md` | §3.1–§3.6 Index queue, content extraction, embedding generation, entity extraction, summaries, SemanticMetadata |
| Space Indexer indexing policy | `docs/intelligence/space-indexer/indexing-policy.md` | §4.1–§4.4 Full-text vs embedding split, promotion criteria, on-demand embedding, batch re-indexing |
| Space Indexer embedding index | `docs/intelligence/space-indexer/embedding-index.md` | §5.1–§5.6 HNSW graph, quantization (SQ8/PQ/RaBitQ), persistence, eviction, filtered search |
| Space Indexer full-text index | `docs/intelligence/space-indexer/fulltext-index.md` | §6.1–§6.6 Inverted index, BM25 scoring, tokenization (CJK bigrams), maintenance, phrase queries |
| Space Indexer relationship graph | `docs/intelligence/space-indexer/relationship-graph.md` | §7.1–§7.7 Relationship types, graph storage, traversal, PersonalRank, cross-object discovery, edge aging |
| Space Indexer search integration | `docs/intelligence/space-indexer/search-integration.md` | §8.1–§8.3 Semantic interface, RRF/learned score fusion, graceful degradation; §9.1–§9.5 Cross-service integration |
| Space Indexer security | `docs/intelligence/space-indexer/security.md` | §10.1–§10.4 Resource path separation, crash containment, capability-gated access, embedding privacy; §11.1–§11.4 Compute/memory/storage/latency budgets |
| Space Indexer intelligence | `docs/intelligence/space-indexer/intelligence.md` | §12.1–§12.2 AIRS-dependent (adaptive priority, clustering, query optimization), kernel-internal ML (pattern prediction, eviction, Bloom filters) |
| Tool Manager (hub) | `docs/intelligence/tool-manager.md` | §1 Core Insight, §2 Architecture, §13 Impl Order, §14 Design Principles, Document Map |
| Tool registry & schema | `docs/intelligence/tool-manager/registry.md` | §3.1–§3.4 ToolId, RegisteredTool, ToolRegistry, §4.1–§4.4 Schema system, discovery, versioning |
| Tool execution pipeline | `docs/intelligence/tool-manager/execution.md` | §5.1–§5.7 Seven-stage pipeline, 3-level capability validation, §6.1–§6.4 Timeout, cancellation, errors |
| Tool sandboxing | `docs/intelligence/tool-manager/sandboxing.md` | §7.1–§7.3 Process isolation, resource limits, capability attenuation, §8.1–§8.3 Crash containment |
| Tool interop & MCP | `docs/intelligence/tool-manager/interop.md` | §9.1–§9.7 Multi-runtime bridging (Rust/Python/TS/WASM), §10.1–§10.5 MCP alignment, bridge, portability |
| Tool security & audit | `docs/intelligence/tool-manager/security.md` | §11.1–§11.4 Capability enforcement, trust levels, rate limiting, §12.1–§12.4 Audit, metrics, tracing |
| Tool AI intelligence | `docs/intelligence/tool-manager/intelligence.md` | §15.1–§15.4 AI-native tool selection, §16.1–§16.3 Kernel-internal ML, §17.1–§17.7 Future directions |
| Runtime Advisor (hub) | `docs/intelligence/runtime-advisor.md` | §1 Core Insight, §2 Architecture, §11 Design Principles, §12 Impl Order, Document Map |
| Runtime Advisor scheduling | `docs/intelligence/runtime-advisor/scheduling.md` | §3 AIRS learning frontend, §4 Kernel scheduler backend |
| Runtime Advisor allocation | `docs/intelligence/runtime-advisor/allocation.md` | §5 AIRS lifetime prediction, §6 Kernel slab integration |
| Runtime Advisor GC scheduling | `docs/intelligence/runtime-advisor/gc-scheduling.md` | §7 AIRS RL-based GC policy, §8 Runtime GC hook integration |
| Runtime Advisor anomaly detection | `docs/intelligence/runtime-advisor/anomaly-detection.md` | §9 Three detection layers, §10 Response pipeline |
| Behavioral monitor (hub) | `docs/intelligence/behavioral-monitor.md` | §1 Core Insight, §2 Architecture, §14 Impl Order, §15 Design Principles, Document Map |
| Behavioral data model | `docs/intelligence/behavioral-monitor/data-model.md` | §3.1–§3.7 BehavioralMonitor, baselines, policies, hard limits, anomaly types, state byte, storage |
| Behavioral detection | `docs/intelligence/behavioral-monitor/detection.md` | §4.1–§4.5 Statistical detection (Welford/z-score), §5.1–§5.4 Baseline learning |
| Behavioral response | `docs/intelligence/behavioral-monitor/response.md` | §6.1–§6.5 Escalation/enforcement, §7.1–§7.3 Provenance/audit |
| Behavioral profiling | `docs/intelligence/behavioral-monitor/profiling.md` | §8.1–§8.5 Agent behavior profiling pipeline |
| Behavioral security | `docs/intelligence/behavioral-monitor/security.md` | §9.1–§9.5 Layer integration, §10.1–§10.3 AIRS self-monitoring |
| Behavioral evasion | `docs/intelligence/behavioral-monitor/evasion.md` | §11.1–§11.6 Evasion resistance, adversarial robustness |
| Behavioral intelligence | `docs/intelligence/behavioral-monitor/intelligence.md` | §12.1–§12.4 Kernel-internal ML, §13.1–§13.5 AIRS-dependent, §16 Future directions |
| Conversation Manager (hub) | `docs/intelligence/conversation-manager.md` | §1 Overview, §2 Architecture, §15 Impl Order, §16 Design Principles, Document Map |
| Conversation sessions & persistence | `docs/intelligence/conversation-manager/sessions.md` | §3 Session lifecycle/pool/routing, §4 Storage/search/forking/retention |
| Context windows & compression | `docs/intelligence/conversation-manager/context-windows.md` | §5 Assembly pipeline/token budget/RAG, §6 Compression tiers/multi-model transfer |
| Tool orchestration | `docs/intelligence/conversation-manager/tool-orchestration.md` | §7 Tool discovery/invocation/chains, §8 Built-in tools (space/system/Flow) |
| Conversation Bar | `docs/intelligence/conversation-manager/conversation-bar.md` | §9 Bar design/invocation/accessibility, §10 Structured output, §11 Compositor/context/multi-conversation |
| Streaming token delivery | `docs/intelligence/conversation-manager/streaming.md` | §12 Streaming architecture/backpressure/cancellation/tool detection, §13 AI-native streaming intelligence |
| Conversation security | `docs/intelligence/conversation-manager/security.md` | §14 Injection defense/capabilities/privacy/audit/content safety |
| Context engine (hub) | `docs/intelligence/context-engine.md` | §1 Overview, §2 Architecture, §11 Impl Order, §12 Design Principles, Document Map |
| Context signals | `docs/intelligence/context-engine/signals.md` | §3 Signal sources, weights, collection frequency |
| Context inference | `docs/intelligence/context-engine/inference.md` | §4 Feature extraction, classifier, hysteresis, transitions |
| Context overrides | `docs/intelligence/context-engine/overrides.md` | §5 Override types, rules, API |
| Context consumers | `docs/intelligence/context-engine/consumers.md` | §6 Scheduler, attention manager, compositor, preference service |
| Context learning & AI | `docs/intelligence/context-engine/learning.md` | §7 Learning, §8 Fallback, §13 AI-native context intelligence, §14 Future directions |
| Context SDK & diagnostics | `docs/intelligence/context-engine/sdk.md` | §9 SDK API, §10 Diagnostics & Inspector |
| Attention management | `docs/intelligence/attention.md` | §1–§17 Core design (Phase 17+), §18 Security, §19 AI-Native Intelligence, §20 Testing Strategy, §21 Future Directions |
| Task manager | `docs/intelligence/task-manager.md` | §1-§13 Core (Phase 17+), §14 Security, §15 Observability, §16 Multi-device, §17 Power/thermal, §18 AI-native AIRS, §19 Kernel-internal ML, §20 Future, §21 Cross-refs |
| Preferences (hub) | `docs/intelligence/preferences.md` | §1 Overview, §2 Architecture, §19 Impl Order, §20 Design Principles, Document Map |
| Preference data model | `docs/intelligence/preferences/data-model.md` | §3.1–§3.5 Preference types, values, sources (Enterprise/Context-driven), metadata, schema registry |
| Preference resolution | `docs/intelligence/preferences/resolution.md` | §4.1–§4.4 7-tier source precedence, §5.1–§5.2 NLU pipeline, §10.1–§10.3 Conflict detection/resolution |
| Preference inference | `docs/intelligence/preferences/inference.md` | §6.1–§6.3 Behavioral observer, §7.1–§7.2 Change propagation, §8.1–§8.3 Agent preferences/SDK |
| Preference history | `docs/intelligence/preferences/history.md` | §9.1–§9.2 Explainability/undo, §11.1–§11.2 Cross-device sync, §12.1–§12.6 Categories/defaults, §13 Settings UI |
| Preference temporal rules | `docs/intelligence/preferences/temporal.md` | §14.1–§14.7 Context-driven rules: time-of-day, location, activity, device-presence triggers |
| Preference security | `docs/intelligence/preferences/security.md` | §15.1–§15.7 Capability-gated access, trust levels, enterprise policy, rate limiting, audit, privacy |
| Preference intelligence | `docs/intelligence/preferences/intelligence.md` | §16.1–§16.5 AIRS-dependent (contextual bandits, NLU, anomaly), §17.1–§17.5 Kernel-internal ML (pattern detection, confidence, conflict prediction, feature importance, model budget) |
| Preference testing | `docs/intelligence/preferences/testing.md` | §18.1–§18.5 Unit, integration, property-based, fuzz, QEMU validation |
| Agent framework (hub) | `docs/applications/agents.md` | §1 Core Insight, §15 Impl Order, §16 Design Principles, Document Map |
| Agent anatomy & categories | `docs/applications/agents/anatomy.md` | §2 What Is an Agent, §3 Categories, AgentProcess, AgentManifest, Agent Card |
| Agent lifecycle & packages | `docs/applications/agents/lifecycle.md` | §4 Installation & Package Model, §5 Startup/States/Shutdown/Recovery/Updates |
| Agent sandbox & security | `docs/applications/agents/sandbox.md` | §6 Isolation Mechanisms & Syscalls, §7 Security Layer Integration |
| Agent SDK & Scriptable Protocol | `docs/applications/agents/sdk.md` | §8 SDK Architecture & AgentContext, §9 Scriptable Protocol & Language Runtimes |
| Agent communication | `docs/applications/agents/communication.md` | §10 IPC Patterns & Reactive Queries, §11 Service Discovery, Content Types, URL Schemes |
| Agent distribution | `docs/applications/agents/distribution.md` | §12 Agent Store & Package Format, §13 Testing & Development Tools |
| Agent resources | `docs/applications/agents/resources.md` | §14 Memory/CPU/Network/Inference Budgets & Resource Accounting |
| Agent intelligence | `docs/applications/agents/intelligence.md` | §17 Kernel-Internal ML, §18 AIRS-Dependent Intelligence, §19 Future Directions |
| Browser Kit (hub) | `docs/applications/browser.md` | §1 Core Insight, §2 Responsibility Decomposition, §3 Architecture Overview, §14 Design Principles, §15 Impl Order, Document Map |
| Browser SDK traits | `docs/applications/browser/sdk.md` | §4 Browser Kit SDK (7 traits), §5 Web API Bridge |
| Browser origin mapping | `docs/applications/browser/origin-mapping.md` | §6 Origin-to-capability mapping, §7 CORS as capabilities |
| Browser storage bridge | `docs/applications/browser/storage-bridge.md` | §8 Web storage as Spaces |
| Browser engine integration | `docs/applications/browser/engine-integration.md` | §9 Engine integration patterns, §10 Reference browser |
| Browser security | `docs/applications/browser/security.md` | §11 Security architecture, §12 Unique capabilities |
| Browser intelligence | `docs/applications/browser/intelligence.md` | §13 AI-native browser intelligence |
| Inspector (hub) | `docs/applications/inspector.md` | §1 Core Insight, §2 Architecture, §15 Design Principles, §16 Impl Order, §17 Comparisons, Document Map |
| Inspector architecture | `docs/applications/inspector/architecture.md` | §3 Agent Identity, §4 Component Architecture, Data Model, Innovations |
| Inspector views | `docs/applications/inspector/views.md` | §5.1–§5.9 All 9 views with research enhancements |
| Inspector actions | `docs/applications/inspector/actions.md` | §6 User Actions, §7 Conversation Bar, §8 Auto-Open, §9 Performance |
| Inspector threat model | `docs/applications/inspector/threat-model.md` | §10 Threat Model, §11 Security Layer Positioning, Provenance Integrity, Trust Model |
| Inspector intelligence | `docs/applications/inspector/intelligence.md` | §12 AIRS-Dependent, §13 Kernel-Internal ML, §14 Future Directions |
| Inspector testing | `docs/applications/inspector/testing.md` | §18 Testing Strategy, §19 Accessibility |
| Terminal emulator (hub) | `docs/applications/terminal.md` | §1 Core Insight, §2 Architecture, §9–§12 Design/Impl/Future/AI-Native, Document Map |
| Terminal VT emulation | `docs/applications/terminal/emulation.md` | §3.1–§3.7 State machine, escape sequences, modes, charset, grid, colors, reference |
| Terminal rendering | `docs/applications/terminal/rendering.md` | §4.1–§4.7 Font engine, glyph atlas, GPU rendering, damage tracking, scrollback, compositor, performance model |
| Terminal sessions & PTY | `docs/applications/terminal/sessions.md` | §5.1–§5.11 IPC-based PTY, session lifecycle, shell spawning, job control, POSIX bridge, persistence, remote, error handling |
| Terminal input | `docs/applications/terminal/input.md` | §6.1–§6.6 Keyboard flow, VT translation, mouse reporting, selection, secure input, IME |
| Terminal multiplexer | `docs/applications/terminal/multiplexer.md` | §7.1–§7.7 Session broker, pane splitting, detach/reattach, SSH forwarding, reconnection, error recovery |
| Terminal integration | `docs/applications/terminal/integration.md` | §8.1–§8.9 Subsystem framework, capability gate, spaces, Flow, accessibility, audit, power management, Scriptable terminal protocol |
| Terminal testing & performance | `docs/applications/terminal/testing.md` | §13 Testing Strategy, §14 Performance Verification |
| Interface Kit (hub) | `docs/applications/interface-kit.md` | §1 Overview, §2 Architecture, §15 Design Principles, §16 Impl Order, Document Map |
| Interface Kit application model | `docs/applications/interface-kit/application-model.md` | §3 Elm Architecture, Widget trait, InterfaceCommand system |
| Interface Kit widgets | `docs/applications/interface-kit/widgets.md` | §4 Widget library (30+ widgets), custom widgets |
| Interface Kit layout | `docs/applications/interface-kit/layout.md` | §5 Constraint-based layout, responsive, incremental layout, grid |
| Interface Kit theme | `docs/applications/interface-kit/theme.md` | §6 Design tokens, context-aware themes, motion tokens, elevation |
| Interface Kit text | `docs/applications/interface-kit/text.md` | §7 Text pipeline, font fallback, glyph cache, i18n (ICU4X), variable fonts |
| Interface Kit rendering | `docs/applications/interface-kit/rendering.md` | §8 Render pipeline, display list, damage tracking, animation system |
| Interface Kit backends | `docs/applications/interface-kit/backends.md` | §9 Platform backends, bridge trait, AIOS/Linux/macOS/Web |
| Interface Kit AIOS features | `docs/applications/interface-kit/aios-features.md` | §10 Semantic hints, Flow integration, Space persistence, capability-aware UI |
| Interface Kit development | `docs/applications/interface-kit/development.md` | §11 SDK integration, §14 CI/CD, testing strategy |
| Interface Kit accessibility | `docs/applications/interface-kit/accessibility.md` | §12 Accessibility tree (AccessKit), screen reader, keyboard nav, reduced motion |
| Interface Kit performance | `docs/applications/interface-kit/performance.md` | §13 Frame budget, texture atlas, performance guidelines |
| Interface Kit intelligence | `docs/applications/interface-kit/intelligence.md` | §17 AIRS-dependent UI, §18 Kernel-internal ML, §19 Future directions |
| Security model (hub) | `docs/security/model.md` | §1 Threat model, §12 Impl order, Document Map |
| Security defense layers | `docs/security/model/layers.md` | §2 Eight security layers deep dive |
| Capability system internals | `docs/security/model/capabilities.md` | §3.1–§3.6 Token lifecycle, kernel table, attenuation, delegation, temporal caps |
| Composable capability profiles | `docs/security/model/capabilities.md` | §3.7 (Phase 45) |
| Crypto, ARM HW security, testing | `docs/security/model/hardening.md` | §4 Crypto, §5 ARM HW, §8 Testing |
| Security operations & zero trust | `docs/security/model/operations.md` | §6 Events, §7 Audit, §9 AIRS, §10 Zero trust, §11 Comparisons, §13 Future |
| AIRS capability intelligence | `docs/intelligence/airs/intelligence-services.md` | §5.9 (Phase 46) |
| Intent verifier (hub) | `docs/intelligence/intent-verifier.md` | §1 Core Insight, §14 Impl Order, §15 Design Principles, Document Map |
| Intent verifier pipeline | `docs/intelligence/intent-verifier/pipeline.md` | §2 Architecture, §4 Verification Pipeline, §10 Performance Model |
| Intent specification | `docs/intelligence/intent-verifier/specification.md` | §3 DeclaredIntent + StructuredIntent |
| Information flow verification | `docs/intelligence/intent-verifier/information-flow.md` | §5 IPC Taint Labels (DIFC), Data Flow Graph, Exfiltration Detection |
| Behavioral integration | `docs/intelligence/intent-verifier/behavioral.md` | §6 Layer 1+3 Coordination, §9 Temporal Logic Monitor (MTL) |
| Intent verifier security | `docs/intelligence/intent-verifier/security.md` | §7 Capability Integration, §8 Adversarial Resistance, §11 Graceful Degradation |
| Intent verifier intelligence | `docs/intelligence/intent-verifier/intelligence.md` | §12 Testing, §13 AI-Native Intelligence, §16 Future Directions, §17 References |
| Decentralisation (cross-cutting) | `docs/security/decentralisation.md` | §1 Core Insight, §2 Architecture, §3-§7 Five Pillars, §8 Trust Model, §9 Threat Model, §10 Inspector, §11 Offline-First, §12 Comparisons, §13 Roadmap, §15 Principles |
| Adversarial defense (hub) | `docs/security/adversarial-defense.md` | §1 Core Insight, §16 Impl Order, §17 Design Principles, Document Map |
| Adversarial threat model | `docs/security/adversarial-defense/threat-model.md` | §2 Threat taxonomy (direct/indirect injection, jailbreak, multi-agent, ML evasion, supply chain), §3 Attack surface map |
| Control/data plane separation | `docs/security/adversarial-defense/control-data-separation.md` | §4 Instruction sources, data labeling, enforcement points, label integrity, limitations |
| Adversarial screening pipeline | `docs/security/adversarial-defense/screening.md` | §5 Input screening (pattern + ML), §6 Output validation, §7 Hint screening |
| Adversarial detection & response | `docs/security/adversarial-defense/response.md` | §8 Detection/response pipeline, §9 Forensics & incident reconstruction |
| Adversarial defense intelligence | `docs/security/adversarial-defense/intelligence.md` | §10 Kernel-internal ML, §11 AIRS-dependent intelligence, §12 Future directions |
| Adversarial defense testing | `docs/security/adversarial-defense/testing.md` | §13 Testing/verification, §14 POSIX compatibility, §15 Cross-reference index |
| Fuzzing & input hardening (hub) | `docs/security/fuzzing.md` | §1 Overview, §2 Attack surface, Document Map |
| Fuzzing hardening strategies | `docs/security/fuzzing/strategies.md` | §3.1–3.7 Language, syscall, memory, IPC, driver, manifest, concurrency |
| Fuzzing adoption roadmap | `docs/security/fuzzing/adoption-roadmap.md` | §4.1–4.7 Phased adoption (host-side through formal verification) |
| Fuzzing tooling & catalog | `docs/security/fuzzing/tooling.md` | §5.1–5.4 Tiered tooling, §6 Fuzz target catalog |
| Fuzzing AI-native strategies | `docs/security/fuzzing/ai-native.md` | §7.1–7.3 Dev-time AI, kernel-internal AI, AIRS-dependent |
| Secure boot & updates (hub) | `docs/security/secure-boot.md` | §1 Core Insight, §14 Impl Order, §15 Design Principles, Document Map |
| Secure boot threat model & trust chain | `docs/security/secure-boot/trust-chain.md` | §2 Threat model, §3 Six-link chain of trust, §3.7 Measured boot, §3.8 Remote attestation |
| UEFI Secure Boot & TrustZone | `docs/security/secure-boot/uefi.md` | §4 UEFI integration/signing/verification, §5 TrustZone key migration/sealing/counters/OP-TEE path |
| A/B updates & rollback | `docs/security/secure-boot/updates.md` | §6 A/B scheme, §7 Delta updates, §8 Update channels (system/agent/model), §9 Rollback protection |
| Update security operations | `docs/security/secure-boot/operations.md` | §10 Capabilities/verification/audit/incident/revocation, §11 POSIX compatibility |
| Secure boot AI intelligence | `docs/security/secure-boot/intelligence.md` | §12 AI-native (model integrity/scheduling/anomaly), §13 Kernel-internal ML, §16 Future directions |
| Static analysis & formal verification | `docs/security/static-analysis.md` | All (all phases) |
| Privacy architecture (hub) | `docs/security/privacy.md` | §1 Core Insight, §2 Architecture, §15 Impl Order, §16 Design Principles, Document Map |
| Agent privacy model | `docs/security/privacy/agent-privacy.md` | §3.1–§3.3 Privacy manifests/budgets/taint, §4.1–§4.3 Collusion detection/budget aggregation |
| Sensor & hardware privacy | `docs/security/privacy/sensor-privacy.md` | §5.1–§5.4 Sensor coordinator/camera/audio/location, §6.1–§6.3 Kill switches/consent/revocation |
| Data lifecycle privacy | `docs/security/privacy/data-lifecycle.md` | §7.1–§7.4 Classification/retention/scrubbing/erasure, §8.1–§8.3 Encryption/DLP/cross-zone |
| AI privacy | `docs/security/privacy/ai-privacy.md` | §9.1–§9.4 Inference/provenance/ML/embeddings, §10.1–§10.2 Prompt injection/screening |
| Privacy intelligence | `docs/security/privacy/intelligence.md` | §11.1–§11.3 Anomaly/prediction/PII detection, §12.1–§12.4 AIRS adaptation/scoring/queries/future |
| Privacy testing & verification | `docs/security/privacy/testing.md` | §13.1–§13.3 Property/regression/red-team testing, §14.1–§14.2 POSIX bridge/cross-reference |
| Experience layer | `docs/experience/experience.md` | §1 Core Insight, §2 Five Surfaces, §3-§11 Surface details, §12 First Boot, §13 Settings, §14 Multi-Device, §15 Security UX, §16 Developer, §17 Design Language, §18 What Users Never See, §19 AI-Native, §20 Future, §21 Impl Order (Phase 7+) |
| Accessibility (hub) | `docs/experience/accessibility.md` | §1 Overview, §2 Architecture, §12 Impl Order, §13 Design Principles, Document Map |
| Assistive technology | `docs/experience/accessibility/assistive-technology.md` | §3 Screen reader (eSpeak-NG), §4 Braille display, §5 Switch scanning, §6 High contrast/magnification, §7 Voice control |
| Accessibility system integration | `docs/experience/accessibility/system-integration.md` | §8 Boot-time accessibility, §9 Accessibility tree |
| Accessibility AI enhancement | `docs/experience/accessibility/ai-enhancement.md` | §10 AIRS enhancement matrix, §11 No-AIRS fallback specifications |
| Accessibility intelligence | `docs/experience/accessibility/intelligence.md` | §14 Kernel-internal ML, §15 AIRS-dependent intelligence, §17 Future directions |
| Accessibility testing | `docs/experience/accessibility/testing.md` | §16 Testing strategy, WCAG validation, adversarial testing |
| Accessibility security | `docs/experience/accessibility/security.md` | §18 Security and privacy, §19 Cross-reference index |
| Identity (hub) | `docs/experience/identity.md` | §1 Overview, §2 Architecture, §15 Impl Order, §16 Design Principles, Document Map |
| Identity core & keys | `docs/experience/identity/core.md` | §3 Identity data model, §4 PQC key hierarchy, CryptoBackend, HSM, SLIP-0010 |
| Identity relationships & trust | `docs/experience/identity/relationships.md` | §5 TrustRelation, did:peer, §6 EigenTrust, SD-JWT, TOFU, key transparency |
| Identity sharing | `docs/experience/identity/sharing.md` | §7 Space sharing config, share/revoke flows |
| Identity cross-device | `docs/experience/identity/cross-device.md` | §8 Device addition/revocation, §9 Space Mesh sync, peer auth |
| Identity agents | `docs/experience/identity/agents.md` | §10 Manifest signing, supply chain, delegation chains, AI provenance |
| Identity credentials | `docs/experience/identity/credentials.md` | §11 Credential isolation, §12 WebAuthn platform authenticator, §12.6 Service identities, §12.7 OAuth |
| Identity privacy & recovery | `docs/experience/identity/privacy.md` | §13 Selective disclosure (ZKP), §14 Graduated 3-tier recovery (Feldman VSS) |
| Identity intelligence | `docs/experience/identity/intelligence.md` | §17 Kernel-internal ML, AIRS-dependent, comparative analysis |
| Kit architecture (hub) | `docs/kits/README.md` | 30 Kits across 4 layers (Kernel, Platform, Intelligence, Application) |
| Kit docs — Kernel layer | `docs/kits/kernel/{memory,ipc,capability,compute}.md` | Kit API traits, Phase 5 retroactive extraction |
| Kit docs — Platform layer | `docs/kits/platform/{storage,network,input,audio,media,usb,camera,wireless,power,thermal,translation}.md` | Kit API traits extracted inline per phase |
| Kit docs — Intelligence layer | `docs/kits/intelligence/{airs,context,preference,search,flow,intent,attention}.md` | Kit API traits extracted inline per phase |
| Kit docs — Application layer | `docs/kits/application/{app,interface,browser,conversation,identity,notification,security}.md` | Kit API traits extracted inline per phase |
| Kit cookbook | `docs/kits/cookbook.md` | SDK examples, Phase 26 |
| Developer guide | `docs/project/developer-guide.md` | All (all phases) |
| AI agent context | `docs/project/ai-agent-context.md` | All (all phases) |
