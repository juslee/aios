# AIOS Thermal Management

**Audience:** Kernel developers, platform engineers, application developers
**Phase:** 19 (Power Management & Thermal) — preparatory work in Phases 14–18
**Related:** [power-management.md](./power-management.md) — unified power policy engine,
[hal.md](../kernel/hal.md) — platform abstraction layer,
[scheduler.md](../kernel/scheduler.md) — thermal-aware scheduling

---

## §1 Core Insight

Thermal management in AIOS is **proactive system-wide resource allocation**, not reactive throttling. The system continuously monitors die temperatures across all thermal zones, predicts thermal trajectories using gradient analysis and learned models, and adjusts workload distribution and cooling before hardware-forced throttling degrades user experience.

Unlike traditional OSes that treat thermal management as a driver-level concern, AIOS integrates thermal awareness across the full stack: from hardware sensor abstraction through kernel scheduling decisions to application-facing thermal headroom APIs. The Policy Engine (power-management.md §5) serves as the single authority, aggregating thermal sensor readings with battery state, user activity, and AIRS predictions to make globally optimal power-thermal decisions.

**Design philosophy:**

- **Proactive over reactive** — reduce workload at `ThermalState::Warm` (70°C to passive trip) before firmware-forced throttling, keeping UX smooth
- **PID over step-wise** — smooth continuous adjustment via PID governors eliminates oscillation near trip points
- **Platform differences contained** — per-platform sensor drivers and trip points live in HAL implementations; policy logic is platform-agnostic
- **Safety invariants are non-overridable** — `Critical` trip enforcement cannot be disabled by any capability; kernel always protects hardware
- **AI-native from the ground up** — kernel-internal ML handles immediate thermal decisions; AIRS provides strategic thermal intelligence

---

## Document Map

| Document | Sections | Content |
|---|---|---|
| **This file** | §1, §14–§15 | Overview, implementation order, design principles |
| [zones.md](./thermal/zones.md) | §2–§3 | Thermal zone abstraction, sensors, trip points, polling, filtering, coupling |
| [cooling.md](./thermal/cooling.md) | §4–§5 | Cooling device trait, DVFS, fans, governors (step-wise, PID, bang-bang) |
| [scheduling.md](./thermal/scheduling.md) | §6–§7 | Scheduler integration, WCET scaling, dark silicon budgeting, core-idling |
| [platform-drivers.md](./thermal/platform-drivers.md) | §8 | Per-platform drivers: QEMU, Pi 4, Pi 5, Apple Silicon, ARM SCMI |
| [integration.md](./thermal/integration.md) | §9–§10 | Subsystem coordination (GPU, audio, storage, network), POSIX bridge, agent API |
| [security.md](./thermal/security.md) | §11 | Capability-gated access, audit trail, safety invariants, formal verification |
| [intelligence.md](./thermal/intelligence.md) | §12–§13 | Kernel-internal ML, AIRS thermal advisor, anomaly detection, future directions |

---

## §14 Implementation Order

### Preparatory Work (Phases 14–18)

| Phase | Thermal Preparatory | Dependency |
|---|---|---|
| 14 | Scheduler thermal throttling hooks (`ThermalState` enum, WCET scaling) | Scheduler complete |
| 16 | Subsystem framework `PowerManaged` trait with thermal coordination points | Subsystem framework |
| 18 | Wireless power management with thermal sensor polling infrastructure | Device model |

### Phase 19: Thermal Management (Weeks 75–78)

| Milestone | Steps | Target | Observable Result |
|---|---|---|---|
| M58 | Thermal zone + sensor HAL | Week 75–76 | `read_temperature()` returns millidegrees on all platforms |
| M59 | Cooling devices + governors | Week 76–77 | PID governor smoothly controls frequency; fan PWM on Pi 5 |
| M60 | Integration + security + AI | Week 77–78 | Agent thermal headroom API; audit trail; thermal prediction |

### Post-Phase 19

| Phase | Enhancement |
|---|---|
| 25 | GPU co-scheduling with thermal-aware placement |
| 27 | Formal verification of thermal state machine safety properties |
| 29 | AIRS thermal advisor: GNN prediction, multi-agent RL budget coordination |

---

## §15 Design Principles

1. **One authority** — the Policy Engine (power-management.md §5) makes all thermal decisions. Subsystems report readings; they do not independently throttle.

2. **Sensors are passive** — thermal zone drivers read temperatures and report. They never initiate cooling actions. The governor decides.

3. **Guards prevent harm** — `Critical` trip point enforcement is a kernel invariant. No capability, no governor override, no AIRS suggestion can raise or disable a Critical trip point. The kernel always initiates orderly shutdown at Critical.

4. **PID is the default** — step-wise governors are available as a fallback, but PID (Intelligent Power Allocation) is the preferred governor for zones with energy model data. PID eliminates the oscillation that step-wise governors exhibit near trip boundaries.

5. **Platform code is isolated** — all platform-specific logic lives in `PlatformThermal` trait implementations. Adding a new platform means implementing one trait, not modifying thermal policy.

6. **Proactive is better than reactive** — the system should throttle gently at `Warm` (70–78°C) rather than face firmware-forced hard throttling at `Passive` (80°C). Users experience smooth performance transitions, not abrupt drops.

7. **Thermal coupling is modeled** — on multi-zone platforms (Pi 5, Apple Silicon), the system models heat transfer between zones. GPU thermal load affects CPU temperature predictions.

8. **Agents can cooperate** — the `ThermalHeadroom` API lets agents query remaining thermal budget and voluntarily reduce workload before the kernel forces throttling.

9. **Everything is audited** — every trip point crossing, governor decision, cooling state change, and capability-gated access is logged to the thermal audit trail.

10. **ML augments, never replaces** — kernel-internal ML (frozen decision trees, lightweight NNs) provides hints to governors. The governor always makes the final decision and can override ML suggestions.

---

## Cross-Reference Index

| Section | Sub-file | External References |
|---|---|---|
| §2.1 ThermalZone | [zones.md](./thermal/zones.md) | hal.md §17, power-management.md §4.2 |
| §3.1 Trip points | [zones.md](./thermal/zones.md) | power-management.md §5.2, §6.2 |
| §4.1 CoolingDevice | [cooling.md](./thermal/cooling.md) | device-model/lifecycle.md §7.5 |
| §5.3 PID governor | [cooling.md](./thermal/cooling.md) | power-management.md §14.2 |
| §6.1 ThermalState | [scheduling.md](./thermal/scheduling.md) | scheduler.md §8.4 |
| §6.3 WCET scaling | [scheduling.md](./thermal/scheduling.md) | scheduler.md §8.4, §11.2 |
| §7.2 Dark silicon | [scheduling.md](./thermal/scheduling.md) | power-management.md §14.3 |
| §8.1–§8.4 Platforms | [platform-drivers.md](./thermal/platform-drivers.md) | power-management.md §9, hal.md §17 |
| §8.5 SCMI | [platform-drivers.md](./thermal/platform-drivers.md) | device-model.md §5 |
| §9.1 GPU thermal | [integration.md](./thermal/integration.md) | gpu/integration.md §17.2.4 |
| §10.2 Headroom API | [integration.md](./thermal/integration.md) | agents.md §10 |
| §11.1 Capability gate | [security.md](./thermal/security.md) | security/model/capabilities.md §3 |
| §12 Kernel ML | [intelligence.md](./thermal/intelligence.md) | power-management.md §14.5–§14.6 |
| §13 AIRS thermal | [intelligence.md](./thermal/intelligence.md) | intelligence/airs.md §5 |
