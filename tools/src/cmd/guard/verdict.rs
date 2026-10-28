//! The guard's decision (`Verdict`, git-push-guard.py L958-981), with the rule
//! tags the shadow logs for coverage (plan step 7b).

use std::collections::BTreeSet;

/// A decision level; the strictest one wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    None,
    Ask,
    Deny,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::Ask => "ask",
            Level::Deny => "deny",
        }
    }
}

/// Which rule made a decision: `push` (git push), `git` (other git commands),
/// `gh`, `sed`, `shell` (what the guard cannot see), `r1`..`r4` (rule 11's team
/// rules), `parse-fallback` (text the guard cannot parse) and `error` (an
/// unreadable payload or an internal error).
pub type Tag = &'static str;

pub const PUSH: Tag = "push";
pub const GIT: Tag = "git";
pub const GH: Tag = "gh";
pub const SED: Tag = "sed";
pub const SHELL: Tag = "shell";
pub const R1: Tag = "r1";
pub const R2: Tag = "r2";
pub const R3: Tag = "r3";
pub const R4: Tag = "r4";
pub const PARSE_FALLBACK: Tag = "parse-fallback";
pub const ERROR: Tag = "error";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub level: Level,
    /// Distinct reasons, in the order they were first given.
    pub reasons: Vec<String>,
    /// A deny that the main-is-user-only note explains (`push_deny`).
    pub push_deny: bool,
    /// The rules behind every decision above `none`.
    pub tags: BTreeSet<Tag>,
}

impl Default for Verdict {
    fn default() -> Verdict {
        Verdict {
            level: Level::None,
            reasons: Vec::new(),
            push_deny: false,
            tags: BTreeSet::new(),
        }
    }
}

impl Verdict {
    pub fn add(&mut self, tag: Tag, level: Level, reason: impl Into<String>) {
        let reason = reason.into();
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
        self.tags.insert(tag);
        if level > self.level {
            self.level = level;
        }
    }

    pub fn ask(&mut self, tag: Tag, reason: impl Into<String>) {
        self.add(tag, Level::Ask, reason);
    }

    /// A deny from the push, merge and gh rules, explained by the
    /// main-is-user-only note (`deny`).
    pub fn deny(&mut self, tag: Tag, reason: impl Into<String>) {
        self.push_deny = true;
        self.add(tag, Level::Deny, reason);
    }

    /// A deny from a team rule (rule 11): the reason stands on its own
    /// (`rule_deny`).
    pub fn rule_deny(&mut self, tag: Tag, reason: impl Into<String>) {
        self.add(tag, Level::Deny, reason);
    }
}
