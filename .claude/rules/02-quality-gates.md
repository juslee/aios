# Quality Gates

Every milestone must pass all applicable gates:

| Gate | Command | Passes when |
|---|---|---|
| Compile | `cargo build --target aarch64-unknown-none` | Zero warnings |
| Check | `just check` (fmt-check + clippy + build) | Zero warnings, zero errors |
| Test | `just test` (host-side unit tests) | All pass |
| QEMU | `just run` | Expected UART string matches phase acceptance criteria |
| CI | Push to GitHub | All CI jobs pass |
| Objdump | `cargo objdump -- -h` | Sections at expected addresses |
| EL | Boot diagnostics | EL = 1, core ID = 0 |

Never mark a milestone complete if any gate fails.

## Post-Implementation Audit Loop (MANDATORY)

Before creating any PR, run doc audit + code review + security/bug review recursively until all three return 0 issues:

1. **Doc audit**: Cross-reference errors, technical accuracy, naming consistency in all modified docs
2. **Code review**: Convention compliance, unsafe documentation, W^X, naming, dead code
3. **Security/bug review**: Logic errors, address confusion (virt vs phys), PTE bit correctness, race conditions

Fix all genuine issues found, commit, and re-run all three audits. Repeat until a full round returns 0 issues across all three categories.
