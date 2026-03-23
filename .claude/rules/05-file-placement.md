# File Placement Rules

```
kernel/src/arch/aarch64/       aarch64-specific code (uart, exceptions, gic, timer, mmu, psci, trap, boot.S, linker.ld)
kernel/src/platform/           Platform trait + per-board implementations (qemu.rs)
kernel/src/mm/                 Memory management (bump, buddy, slab, pools, frame, pgtable, kmap, etc.)
kernel/src/observability/      Structured logging, metrics, trace points
kernel/src/sched/              Scheduler
kernel/src/ipc/                IPC channels, shared memory, notifications, select
kernel/src/cap/                Capability system
kernel/src/task/               Thread/process data structures
kernel/src/service/            Service manager
kernel/src/syscall/            Syscall dispatch and handlers
kernel/src/drivers/            Device drivers
kernel/src/storage/            Storage subsystem (Block Engine, WAL, Object Store, etc.)
kernel/src/                    Platform-agnostic kernel logic
shared/src/                    Types crossing kernel/stub boundary
uefi-stub/src/                 UEFI stub code
docs/phases/                   Phase implementation docs (NN-name.md, flat, no subdirs)
docs/knowledge/decisions/      Architecture Decision Records
docs/knowledge/lessons/        Hard-won lessons and gotchas
docs/knowledge/plans/          Working implementation plans (ephemeral)
docs/knowledge/discussions/    Design explorations (semi-permanent)
```

Architecture docs (`docs/kernel/`, `docs/platform/`, etc.) are for finalized design only. Use `docs/knowledge/` for in-progress work.
