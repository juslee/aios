//! The docs-check checks. Each check is a unit struct implementing [`Check`];
//! [`registry`] lists the implemented checks in CHECK_ORDER (check.py `CHECK_FUNCS`,
//! L1379-1395 at 33c6b3d).

pub mod doc_map;
pub mod harness;
pub mod just_recipes;
pub mod knowledge;
pub mod layout;
pub mod links;
pub mod lock_order;
pub mod milestones;
pub mod pointer_doctor;
pub mod repo_paths;
pub mod test_count;

use crate::cmd::docs_check::{model::Finding, repo::Repo};

/// One docs drift check.
pub trait Check {
    /// Name as in CHECK_ORDER, e.g. "md-links".
    fn name(&self) -> &'static str;
    /// `--list-checks` description.
    fn describe(&self) -> &'static str {
        crate::cmd::docs_check::model::description(self.name())
    }
    /// Findings in check.py's production order; `Err` downcasting to `Skip` when the check cannot run.
    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>>;
}

/// Every docs-check check, in CHECK_ORDER (check.py `CHECK_FUNCS`, L1379-1395 at 33c6b3d).
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
        Box::new(doc_map::DocMap),
        Box::new(repo_paths::RepoPaths),
        Box::new(just_recipes::JustRecipes),
        Box::new(test_count::TestCount),
        Box::new(lock_order::LockOrder),
        Box::new(milestones::MilestoneStatus),
        Box::new(milestones::PhaseCount),
        Box::new(layout::Layout),
        Box::new(harness::HarnessTables),
        Box::new(pointer_doctor::PointerDoctor),
        Box::new(knowledge::KnowledgeHygiene),
    ]
}

#[cfg(test)]
mod registry_tests {
    use super::registry;
    use crate::cmd::docs_check::model::CHECK_ORDER;
    use crate::cmd::docs_check::output::render_list_checks;

    /// `python3 scripts/docs/check.py --list-checks` output (check.py L1597-1600), byte for byte.
    const CHECK_PY_LIST_CHECKS: &str = "\
md-links           relative [text](path) links resolve to a tracked file or directory
section-refs       [x.md](path) §N resolves to a numbered heading (hub subfolders included)
anchors            #fragment links resolve to a GitHub-style heading slug
wiki-links         [[Note]] links resolve to a note in the docs/ vault
doc-map            doc-map.md paths exist and every architecture doc is listed
repo-paths         backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)
just-recipes       backticked `just X` recipes exist; public recipes are documented
test-count         stated host test counts match #[test] in shared/src
lock-order         production Mutex statics vs deadlock-prevention.md §3.3-3.4 and CLAUDE.md
milestone-status   merged 'Phase N MK:' milestones vs phase docs, README, development-plan
phase-count        phase counts in prose match the development-plan §8 table
layout             kernel/src and shared/src modules vs CLAUDE.md layout and rule 05
harness-tables     CLAUDE.md skills/agents tables and layout lists vs .claude/ (plugin skills as plugin:skill)
pointer-doctor     CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/
knowledge-hygiene  docs/knowledge naming, frontmatter, and an empty plans/ dir
";

    #[test]
    fn registry_follows_check_order() {
        let names: Vec<&str> = registry().iter().map(|check| check.name()).collect();
        assert_eq!(names, CHECK_ORDER);
    }

    #[test]
    fn list_checks_matches_check_py() {
        assert_eq!(render_list_checks(&registry()), CHECK_PY_LIST_CHECKS);
    }
}
