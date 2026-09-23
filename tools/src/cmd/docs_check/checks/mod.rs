//! The docs-check checks. Each check is a unit struct implementing [`Check`];
//! [`registry`] lists the implemented checks in CHECK_ORDER (check.py `CHECK_FUNCS`,
//! L1379-1395 at 33c6b3d).

pub mod links;

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

/// Implemented checks in CHECK_ORDER. Each check task appends its checks here, in
/// CHECK_ORDER, so the order of this list is the order of the report.
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
    ]
}
