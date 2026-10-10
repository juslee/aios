# Quality Gates

Every milestone must pass all applicable gates:

| Gate | Command | Passes when |
|---|---|---|
| Compile | `cargo build --target aarch64-unknown-none` | Zero warnings |
| Check | `just check` (fmt-check + clippy + build) | Zero warnings, zero errors |
| Test | `just test` (host-side unit tests) | All pass |
| Tools | `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools --all-targets -- -D warnings && cargo test -p aios-tools` | All pass, when `tools/` changed (`just test` excludes the crate) |
| QEMU | `bash scripts/agent/qemu-lock.sh run ... -- just soak runs=1 secs=75 report_only=1` (`mode=gpu` when the phase needs the display), run by the verifier, or by a solo session's main thread with `--team solo` | `summary.tsv` classifies the boot CLEAN and the log holds the phase's UART lines; exit 77 (deferred under load) is not a pass and not a failure |
| CI | Push to GitHub | All CI jobs pass |
| Objdump | `"$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-objdump -h target/aarch64-unknown-none/debug/kernel` | Sections at expected addresses |
| EL | Boot diagnostics | EL = 1, core ID = 0 |

Never mark a milestone complete if any gate fails.

## Post-Implementation Audit Loop (MANDATORY)

Before a PR is marked ready (`gh pr ready`; PRs open as drafts, rule 03), run `/audit-loop`, on the head the once-per-PR simplifier pass produced (rule 11, Simplify). Each round runs read-only lenses (doc-auditor for docs; code-reviewer with the `rules` and `bugs` lenses for code), and skeptics verify every finding. Only confirmed in-diff findings are fixed on the branch, by a writer; confirmed pre-existing findings become issues. The audit converges on one complete round that confirms nothing. After 4 rounds, or 2 incomplete rounds in a row, the PR stays a draft behind a `needs-human` gate issue.

The lens descriptions:

1. **Doc audit**: Cross-reference errors, technical accuracy, naming consistency in all modified docs
2. **Code review**: Convention compliance, unsafe documentation, W^X, naming, dead code
3. **Security/bug review**: Logic errors, address confusion (virt vs phys), PTE bit correctness, race conditions
