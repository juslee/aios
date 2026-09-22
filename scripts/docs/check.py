#!/usr/bin/env python3
"""Deterministic docs drift checker for AIOS (no LLM, python3 stdlib only).

Files are enumerated with `git ls-files` (tracked plus untracked-but-not-ignored),
never with a filesystem walk, so linked worktrees under .claude/worktrees/ and
build output under target/ are never scanned.

Every finding has a stable key `check|file|target` that does not contain a line
number, and an occurrence count (the number of distinct lines reporting that key).
scripts/docs/baseline.json records the accepted findings. A finding is new when
its key is not in the baseline or it occurs on more lines than the baselined
`count` (default 1); only new findings are reported by default, and the exit
status is 1 when there is at least one. A baseline entry with a `reason` is an
accepted false positive: it is marked '~' and the reason survives
--update-baseline. "New" is relative to the baseline file, not to the branch:
drift that reached main without a baseline update shows as new on every branch
until a docs PR fixes or baselines it.

Usage:
  scripts/docs/check.py                   report new drift only (exit 1 if any)
  scripts/docs/check.py --all             list every finding, new ones marked '+'
  scripts/docs/check.py --json            machine-readable output
  scripts/docs/check.py --markdown        summary for $GITHUB_STEP_SUMMARY
  scripts/docs/check.py --check a,b       run only the named checks
  scripts/docs/check.py --update-baseline rewrite the baseline from current findings
  scripts/docs/check.py --list-checks     print the check names and descriptions
"""

from __future__ import annotations

import argparse
import json
import os
import posixpath
import re
import subprocess
import sys
from dataclasses import dataclass
from urllib.parse import unquote

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

BASELINE_REL = "scripts/docs/baseline.json"

# Docs that describe the current state of the repo. Paths and recipes they
# mention must exist today. Completed-milestone sections of phase docs are
# added separately (see Repo.current_state_regions).
CURRENT_STATE_DOCS = {
    "CLAUDE.md",
    "README.md",
    "CONTRIBUTING.md",
    "docs/project/developer-guide.md",
    "docs/project/agent-loop.md",
}
CURRENT_STATE_PREFIXES = (".claude/",)

# Docs under docs/ that are indexes rather than topics, so doc-map.md need not list them.
DOC_MAP_ALLOWLIST = {"docs/project/doc-map.md"}

# Test-only locks named as excluded in deadlock-prevention.md §3.3.
TEST_LOCKS = {"TEST_CHANNEL", "PI_TEST_CHANNEL"}

# Claude Code tool names accepted in agent `tools:` frontmatter. `mcp__*` names
# are accepted as-is. MultiEdit is deliberately absent: it is not a tool in
# current Claude Code releases.
KNOWN_TOOLS = {
    "Agent", "AskUserQuestion", "Bash", "BashOutput", "CronCreate", "CronDelete",
    "CronList", "Edit", "EnterPlanMode", "EnterWorktree", "ExitPlanMode",
    "ExitWorktree", "Glob", "Grep", "KillShell", "LSP", "ListMcpResourcesTool",
    "Monitor", "NotebookEdit", "PushNotification", "Read", "ReadMcpResourceTool",
    "RemoteTrigger", "SendMessage", "Skill", "Task", "TaskOutput", "TaskStop",
    "TodoWrite", "ToolSearch", "WebFetch", "WebSearch", "Workflow", "Write",
}

# Slash commands that are built into Claude Code rather than project skills.
BUILTIN_COMMANDS = {
    "add-dir", "agents", "clear", "compact", "config", "context", "cost", "doctor",
    "exit", "goal", "help", "hooks", "init", "loop", "mcp", "memory", "model",
    "permissions", "plan", "resume", "review", "schedule", "security-review",
    "simplify", "code-review", "status", "statusline", "tasks", "todos",
}

# Built-in subagent types that need no .claude/agents/ definition.
BUILTIN_AGENTS = {"general-purpose", "Explore", "Plan", "statusline-setup", "output-style-setup"}

KNOWLEDGE_STATUSES = {"draft", "in-progress", "final"}
# docs/knowledge/README.md: discussions are "draft or active", then "graduated".
KNOWLEDGE_STATUSES_EXTRA = {"discussions": {"active", "graduated"}}
KNOWLEDGE_KEYS = ("author", "date", "tags", "status")
KNOWLEDGE_NAME_RE = re.compile(r"^\d{4}-\d{2}-\d{2}-[a-z]{2,3}-[a-z0-9][a-z0-9-]*\.md$")

CHECK_DESCRIPTIONS = {
    "md-links": "relative [text](path) links resolve to a tracked file or directory",
    "section-refs": "[x.md](path) §N resolves to a numbered heading (hub subfolders included)",
    "anchors": "#fragment links resolve to a GitHub-style heading slug",
    "wiki-links": "[[Note]] links resolve to a note in the docs/ vault",
    "doc-map": "doc-map.md paths exist and every architecture doc is listed",
    "repo-paths": "backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)",
    "just-recipes": "backticked `just X` recipes exist; public recipes are documented",
    "test-count": "stated host test counts match #[test] in shared/src",
    "lock-order": "production Mutex statics vs deadlock-prevention.md §3.3-3.4 and CLAUDE.md",
    "milestone-status": "merged 'Phase N MK:' milestones vs phase docs, README, development-plan",
    "phase-count": "phase counts in prose match the development-plan §8 table",
    "layout": "kernel/src and shared/src modules vs CLAUDE.md layout and rule 05",
    "harness-tables": "CLAUDE.md skills/agents tables and layout lists vs .claude/",
    "pointer-doctor": "CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/",
    "knowledge-hygiene": "docs/knowledge naming, frontmatter, and an empty plans/ dir",
}
CHECK_ORDER = list(CHECK_DESCRIPTIONS)

# ---------------------------------------------------------------------------
# Data types
# ---------------------------------------------------------------------------


@dataclass
class Finding:
    """One drift finding. `message` is stored in the baseline, so it must not
    contain line numbers; volatile context (e.g. a source line) goes in `detail`."""

    check: str
    file: str
    target: str
    message: str
    line: int = 0
    also: tuple[int, ...] = ()
    detail: str = ""

    @property
    def key(self) -> str:
        return f"{self.check}|{self.file}|{self.target}"

    @property
    def count(self) -> int:
        """Occurrences: the number of distinct lines reporting this key (1 for file-level findings)."""
        return 1 + len(self.also)

    def location(self) -> str:
        if not self.line:
            return self.file
        extra = f" (also {', '.join(map(str, self.also))})" if self.also else ""
        return f"{self.file}:{self.line}{extra}"

    def text(self) -> str:
        return f"{self.message} ({self.detail})" if self.detail else self.message


class Skip(Exception):
    """Raised by a check that cannot run in this environment (e.g. shallow clone)."""


# ---------------------------------------------------------------------------
# Markdown helpers
# ---------------------------------------------------------------------------

# Any indentation: fences nested in list items are indented past column 3.
FENCE_RE = re.compile(r"^\s*(`{3,}|~{3,})")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
INLINE_LINK_RE = re.compile(
    r"(!?)\[((?:[^\[\]]|\[[^\]]*\])*)\]\(\s*(<[^>]*>|[^)\s]+)(?:\s+(?:\"[^\"]*\"|'[^']*'))?\s*\)"
)
REF_DEF_RE = re.compile(r"^ {0,3}\[([^\]^][^\]]*)\]:\s*(\S+)")  # not footnotes ([^1]: ...)
WIKI_RE = re.compile(r"(!?)\[\[([^\]|#]*)(?:#([^\]|]*))?(?:\|[^\]]*)?\]\]")
SECTION_REF_RE = re.compile(
    r"\[[^\]]*\]\(([^)\s]+\.md)(#[^)]*)?\)\s*\**\s*§\s*([A-Z]?\d+(?:\.\d+)*)"
)
HEADING_NUM_RE = re.compile(r"^(?:§\s*)?([A-Z]?\d+(?:\.\d+)*)(?=[.:\s)]|$)")
SCHEME_RE = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")
PLACEHOLDER_CHARS = ("<", ">", "{", "}", "$", "*", "?", "…", "...", "[", "]")


def mask_code_spans(line: str) -> str:
    """Replace inline code spans (including the backticks) with spaces."""
    out = list(line)
    i = 0
    n = len(line)
    while i < n:
        if line[i] != "`":
            i += 1
            continue
        j = i
        while j < n and line[j] == "`":
            j += 1
        tick = line[i:j]
        close = line.find(tick, j)
        while close != -1 and close + len(tick) < n and line[close + len(tick)] == "`":
            close = line.find(tick, close + len(tick) + 1)
        if close == -1:
            i = j
            continue
        for k in range(i, close + len(tick)):
            out[k] = " "
        i = close + len(tick)
    return "".join(out)


def code_spans(line: str) -> list[str]:
    """Return the contents of inline code spans on a line."""
    spans = []
    i = 0
    n = len(line)
    while i < n:
        if line[i] != "`":
            i += 1
            continue
        j = i
        while j < n and line[j] == "`":
            j += 1
        tick = line[i:j]
        close = line.find(tick, j)
        if close == -1:
            i = j
            continue
        spans.append(line[j:close].strip())
        i = close + len(tick)
    return spans


HTML_COMMENT_RE = re.compile(r"<!--.*?-->")
LIST_ITEM_RE = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+")


def mask_html_comments(line: str) -> str:
    """Replace single-line <!-- ... --> spans with spaces."""
    return HTML_COMMENT_RE.sub(lambda m: " " * len(m.group(0)), line)


def mask_prose(line: str) -> str:
    """Mask code spans and inline HTML comments, keeping column positions."""
    return mask_html_comments(mask_code_spans(line))


def is_escaped(s: str, i: int) -> bool:
    """True when s[i] is preceded by an odd number of backslashes."""
    n = 0
    while i - n - 1 >= 0 and s[i - n - 1] == "\\":
        n += 1
    return n % 2 == 1


def iter_prose_lines(text: str):
    """Yield (lineno, line) for lines outside code blocks and multi-line HTML comments.

    Skipped: fenced blocks at any indentation, multi-line <!-- --> comments, and
    CommonMark indented code blocks (4+ columns after a blank line, outside a list).
    Single-line comments stay in the line; callers mask them with mask_prose().
    """
    fence = None
    in_comment = False
    in_indented = False
    in_list = False
    prev_blank = True
    for lineno, line in enumerate(text.splitlines(), 1):
        m = FENCE_RE.match(line)
        if fence is None and m and not in_indented:
            fence = m.group(1)
            prev_blank = False
            continue
        if fence is not None:
            if m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence) and line.strip() == m.group(1):
                fence = None
            continue
        if in_comment:
            if "-->" in line:
                in_comment = False
            continue
        if line.lstrip().startswith("<!--") and "-->" not in line:
            in_comment = True
            continue
        blank = not line.strip()
        expanded = line.expandtabs(4)
        indent = len(expanded) - len(expanded.lstrip(" "))
        if in_indented:
            if blank or indent >= 4:
                prev_blank = blank
                continue
            in_indented = False
        elif not blank and indent >= 4 and prev_blank and not in_list:
            in_indented = True
            continue
        if not blank:
            if LIST_ITEM_RE.match(line):
                in_list = True
            elif indent == 0 and (prev_blank or line.startswith("#")):
                in_list = False
        prev_blank = blank
        yield lineno, line


def strip_inline_md(text: str) -> str:
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    return text.replace("`", "").replace("**", "").replace("*", "")


def gh_slug(text: str) -> str:
    t = strip_inline_md(text).strip().lower()
    t = re.sub(r"[^\w\- ]", "", t)
    return t.replace(" ", "-")


def headings(text: str) -> list[tuple[int, int, str]]:
    """Return (lineno, level, text) for ATX headings outside code fences."""
    out = []
    for lineno, line in iter_prose_lines(text):
        m = HEADING_RE.match(line)
        if m:
            out.append((lineno, len(m.group(1)), m.group(2)))
    return out


def brace_expand(s: str) -> list[str]:
    m = re.search(r"\{([^{}]*)\}", s)
    if not m:
        return [s]
    out = []
    for part in m.group(1).split(","):
        out.extend(brace_expand(s[: m.start()] + part.strip() + s[m.end():]))
    return out


def section_body(text: str, start_pat: str, stop_pat: str) -> list[tuple[int, str]]:
    """Lines (with numbers) after the first line matching start_pat up to stop_pat."""
    lines = text.splitlines()
    out = []
    started = False
    for i, line in enumerate(lines, 1):
        if not started:
            if re.search(start_pat, line):
                started = True
            continue
        if re.search(stop_pat, line):
            break
        out.append((i, line))
    return out


def table_rows(lines: list[tuple[int, str]]) -> list[tuple[int, list[str]]]:
    rows = []
    for lineno, line in lines:
        s = line.strip()
        if not s.startswith("|"):
            continue
        cells = [c.strip() for c in s.strip("|").split("|")]
        if all(re.fullmatch(r":?-{2,}:?", c) for c in cells if c):
            continue
        rows.append((lineno, cells))
    return rows


def milestone_tokens(text: str) -> set[int]:
    """Milestone numbers mentioned as M12 or as a range M3-M5 / M3–M5."""
    out: set[int] = set()
    for m in re.finditer(r"\bM(\d+)\s*[–-]\s*M(\d+)\b", text):
        lo, hi = int(m.group(1)), int(m.group(2))
        if lo <= hi and hi - lo < 100:
            out.update(range(lo, hi + 1))
    for m in re.finditer(r"\bM(\d+)\b", text):
        out.add(int(m.group(1)))
    return out


# ---------------------------------------------------------------------------
# Repository model
# ---------------------------------------------------------------------------


class Repo:
    def __init__(self, root: str):
        self.root = root
        out = self.git("ls-files", "-z", "--cached", "--others", "--exclude-standard")
        seen = set()
        files = []
        for rel in out.split("\0"):
            if rel and rel not in seen and os.path.lexists(os.path.join(root, rel)):
                seen.add(rel)
                files.append(rel)
        self.files = sorted(files)
        self.file_set = set(self.files)
        self.dir_set = set()
        for f in self.files:
            d = posixpath.dirname(f)
            while d and d not in self.dir_set:
                self.dir_set.add(d)
                d = posixpath.dirname(d)
        self.md_files = [f for f in self.files if f.endswith(".md") and os.path.isfile(os.path.join(root, f))]
        self._text: dict[str, str] = {}
        self._headings: dict[str, list[tuple[int, int, str]]] = {}
        self._merged: dict[int, int] | None = None
        self._merged_error: str | None = None

    def git(self, *args: str, check: bool = True) -> str:
        r = subprocess.run(["git", *args], cwd=self.root, capture_output=True, text=True)
        if check and r.returncode != 0:
            raise RuntimeError(f"git {' '.join(args)} failed: {r.stderr.strip()}")
        return r.stdout

    def text(self, rel: str) -> str:
        if rel not in self._text:
            try:
                with open(os.path.join(self.root, rel), encoding="utf-8", errors="replace") as fh:
                    self._text[rel] = fh.read()
            except OSError:
                self._text[rel] = ""
        return self._text[rel]

    def headings(self, rel: str) -> list[tuple[int, int, str]]:
        if rel not in self._headings:
            self._headings[rel] = headings(self.text(rel))
        return self._headings[rel]

    def exists(self, rel: str) -> bool:
        rel = rel.rstrip("/")
        if rel in ("", "."):
            return True
        if rel in self.file_set or rel in self.dir_set:
            return True
        real = os.path.realpath(os.path.join(self.root, rel))
        if not real.startswith(os.path.realpath(self.root) + os.sep):
            return False
        back = os.path.relpath(real, os.path.realpath(self.root)).replace(os.sep, "/")
        return back in self.file_set or back in self.dir_set

    def resolve(self, src: str, target: str) -> str | None:
        """Resolve a link target relative to src; None if it escapes the repo."""
        if target.startswith("/"):
            path = posixpath.normpath(target.lstrip("/"))
        else:
            path = posixpath.normpath(posixpath.join(posixpath.dirname(src), target))
        if path.startswith("..") or path.startswith("/"):
            return None
        return "" if path == "." else path

    # -- git-derived facts ------------------------------------------------

    def merged_milestones(self) -> dict[int, int]:
        """Milestone number -> phase for 'Phase N MK:' subjects on main's first-parent history."""
        if self._merged is not None:
            return self._merged
        if self._merged_error:
            raise Skip(self._merged_error)
        if self.git("rev-parse", "--is-shallow-repository", check=False).strip() == "true":
            self._merged_error = "shallow clone: git history unavailable (use fetch-depth: 0)"
            raise Skip(self._merged_error)
        base = "HEAD"
        for ref in ("origin/main", "main"):
            if self.git("rev-parse", "--verify", "-q", ref, check=False).strip():
                mb = self.git("merge-base", "HEAD", ref, check=False).strip()
                if mb:
                    base = mb
                    break
        merged: dict[int, int] = {}
        for subject in self.git("log", "--first-parent", "--format=%s", base, check=False).splitlines():
            m = re.match(r"^Phase (\d+) M(\d+):", subject)
            if m:
                merged.setdefault(int(m.group(2)), int(m.group(1)))
        self._merged = merged
        return merged

    def phase_docs(self) -> list[tuple[int, str]]:
        out = []
        for f in self.md_files:
            m = re.match(r"^docs/phases/(\d+)-[^/]+\.md$", f)
            if m:
                out.append((int(m.group(1)), f))
        return sorted(out)

    def milestone_sections(self, rel: str) -> dict[int, tuple[int, int]]:
        """Milestone number -> (first line, last line) of its '## Milestone N' section."""
        lines = self.text(rel).splitlines()
        starts = []
        for lineno, level, text in self.headings(rel):
            if level == 2:
                m = re.match(r"Milestone (\d+)\b", text)
                starts.append((lineno, int(m.group(1)) if m else None))
        out = {}
        for i, (lineno, num) in enumerate(starts):
            if num is None:
                continue
            end = starts[i + 1][0] - 1 if i + 1 < len(starts) else len(lines)
            out[num] = (lineno, end)
        return out

    def current_state_regions(self) -> list[tuple[str, set[int] | None]]:
        """(file, allowed line numbers or None for the whole file) for current-state docs."""
        regions: list[tuple[str, set[int] | None]] = []
        for f in self.md_files:
            if f in CURRENT_STATE_DOCS or f.startswith(CURRENT_STATE_PREFIXES):
                regions.append((f, None))
        try:
            merged = self.merged_milestones()
        except Skip:
            merged = {}
        for _, f in self.phase_docs():
            allowed: set[int] = set()
            for num, (lo, hi) in self.milestone_sections(f).items():
                if num in merged:
                    allowed.update(range(lo, hi + 1))
            if allowed:
                regions.append((f, allowed))
        return regions

    def justfile_recipes(self) -> tuple[set[str], set[str]]:
        """(all recipe names, public recipe names) parsed from the justfile."""
        names: set[str] = set()
        public: set[str] = set()
        private_next = False
        for line in self.text("justfile").splitlines():
            if re.match(r"^\[.*\bprivate\b.*\]", line):
                private_next = True
                continue
            if line.startswith("["):
                continue
            m = re.match(r"^@?([A-Za-z_][A-Za-z0-9_-]*)\b[^:=]*:(?!=)", line)
            if m and not line.startswith((" ", "\t", "#")):
                name = m.group(1)
                names.add(name)
                if not private_next and not name.startswith("_"):
                    public.add(name)
            if line.strip() and not line.startswith("#"):
                private_next = False
        return names, public


# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------


PLACEHOLDER_WORD_RE = re.compile(r"(?<![A-Za-z])(?:N|NN|K|X|XX|XXX|YYYY|MM|DD)(?![A-Za-z])")


def is_placeholder(target: str) -> bool:
    return any(c in target for c in PLACEHOLDER_CHARS)


def is_path_placeholder(path: str) -> bool:
    """Template paths (docs/phases/NN-name.md) and type names (shared/BootInfo) are not paths."""
    if is_placeholder(path) or PLACEHOLDER_WORD_RE.search(path):
        return True
    last = path.rstrip("/").rsplit("/", 1)[-1]
    return not path.endswith("/") and "." not in last and any(ch.isupper() for ch in last)


def iter_links(repo: Repo, rel: str):
    """Yield (lineno, target) for inline links and reference definitions outside code."""
    for lineno, line in iter_prose_lines(repo.text(rel)):
        masked = mask_prose(line)
        for m in INLINE_LINK_RE.finditer(masked):
            if not is_escaped(masked, m.start(2) - 1):  # the '[' (an escaped '!' still leaves a link)
                yield lineno, m.group(3)
        m = REF_DEF_RE.match(masked)
        if m and not is_escaped(masked, masked.index("[")):
            yield lineno, m.group(2)


def split_target(raw: str) -> tuple[str, str] | None:
    """Return (path, fragment) for a local link target, or None to skip it."""
    t = raw.strip()
    if t.startswith("<") and t.endswith(">"):
        t = t[1:-1].strip()
    if not t or SCHEME_RE.match(t) or t.startswith("//"):
        return None
    if is_placeholder(t.split("#", 1)[0]):
        return None
    path, _, frag = t.partition("#")
    path = unquote(path.split("?", 1)[0])
    return path, frag


def check_md_links(repo: Repo) -> list[Finding]:
    out = []
    for rel in repo.md_files:
        for lineno, raw in iter_links(repo, rel):
            parts = split_target(raw)
            if parts is None or not parts[0]:
                continue
            path = repo.resolve(rel, parts[0])
            if path is None or not repo.exists(path):
                out.append(Finding("md-links", rel, parts[0], f"broken link -> {parts[0]}", lineno))
    return out


_SLUG_CACHE: dict[str, set[str]] = {}


def slug_set(repo: Repo, rel: str) -> set[str]:
    if rel in _SLUG_CACHE:
        return _SLUG_CACHE[rel]
    slugs: set[str] = set()
    _SLUG_CACHE[rel] = slugs
    counts: dict[str, int] = {}
    for _, _, text in repo.headings(rel):
        s = gh_slug(text)
        n = counts.get(s, 0)
        counts[s] = n + 1
        slugs.add(s if n == 0 else f"{s}-{n}")
    for m in re.finditer(r"<a\s+(?:name|id)=\"([^\"]+)\"", repo.text(rel)):
        slugs.add(m.group(1).lower())
    return slugs


def check_anchors(repo: Repo) -> list[Finding]:
    out = []
    for rel in repo.md_files:
        for lineno, raw in iter_links(repo, rel):
            t = raw.strip().strip("<>")
            if "#" not in t or SCHEME_RE.match(t):
                continue
            path_part, _, frag = t.partition("#")
            if not frag or is_placeholder(frag) or is_placeholder(path_part):
                continue
            target = rel if not path_part else repo.resolve(rel, unquote(path_part.split("?", 1)[0]))
            if target is None or not target.endswith(".md") or target not in repo.file_set:
                continue
            if unquote(frag).lower() not in slug_set(repo, target):
                out.append(Finding("anchors", rel, t, f"no heading for anchor #{frag} in {target}", lineno))
    return out


def heading_numbers(repo: Repo, rel: str) -> set[str]:
    nums = set()
    for _, _, text in repo.headings(rel):
        m = HEADING_NUM_RE.match(strip_inline_md(text).strip())
        if m:
            nums.add(m.group(1))
    return nums


def hub_members(repo: Repo, hub: str) -> list[str]:
    """Sub-documents of a hub: <dir>/<stem>/*.md plus flat siblings that say 'Part of: [<stem>.md]'."""
    stem = posixpath.splitext(hub)[0]
    base = posixpath.basename(hub)
    members = [f for f in repo.md_files if posixpath.dirname(f) == stem]
    marker = re.compile(r"Part of:\s*\[" + re.escape(base) + r"\]")
    for f in repo.md_files:
        if posixpath.dirname(f) == posixpath.dirname(hub) and f != hub and marker.search(repo.text(f)[:2000]):
            members.append(f)
    return members


def number_resolves(num: str, nums: set[str]) -> bool:
    return num in nums or any(n.startswith(num + ".") for n in nums)


def check_section_refs(repo: Repo) -> list[Finding]:
    out = []
    for rel in repo.md_files:
        for lineno, line in iter_prose_lines(repo.text(rel)):
            masked = mask_prose(line)
            for m in SECTION_REF_RE.finditer(masked):
                if is_escaped(masked, m.start()):
                    continue
                raw, num = m.group(1), m.group(3)
                if is_placeholder(raw) or SCHEME_RE.match(raw):
                    continue
                target = repo.resolve(rel, unquote(raw))
                if target is None or target not in repo.file_set:
                    continue  # md-links reports the missing file
                if number_resolves(num, heading_numbers(repo, target)):
                    continue
                if any(number_resolves(num, heading_numbers(repo, s)) for s in hub_members(repo, target)):
                    continue
                out.append(Finding("section-refs", rel, f"{raw} §{num}", f"no §{num} heading in {target} or its hub members", lineno))
    return out


def check_wiki_links(repo: Repo) -> list[Finding]:
    names: set[str] = set()
    paths: set[str] = set()
    for f in repo.files:
        if f.startswith("docs/"):
            inner = f[len("docs/"):]
            base = posixpath.basename(f)
            names.add(base.lower())
            paths.add(inner.lower())
            if f.endswith(".md"):
                names.add(base[:-3].lower())
                paths.add(inner[:-3].lower())
    out = []
    for rel in repo.md_files:
        for lineno, line in iter_prose_lines(repo.text(rel)):
            masked = mask_prose(line)
            for m in WIKI_RE.finditer(masked):
                if is_escaped(masked, m.start(2) - 2):
                    continue
                note = m.group(2).strip()
                if not note:
                    continue  # [[#heading]] refers to the same note
                key = note.lower()
                if key in names or key in paths or key.removesuffix(".md") in paths:
                    continue
                out.append(Finding("wiki-links", rel, note, f"no note named [[{note}]] in the docs/ vault", lineno))
    return out


def check_doc_map(repo: Repo) -> list[Finding]:
    rel = "docs/project/doc-map.md"
    if rel not in repo.file_set:
        return [Finding("doc-map", rel, "missing", "docs/project/doc-map.md does not exist")]
    out = []
    listed: set[str] = set()
    for lineno, line in iter_prose_lines(repo.text(rel)):
        for span in code_spans(line):
            if not span.startswith("docs/"):
                continue
            for path in brace_expand(span):
                listed.add(path)
                if not repo.exists(path):
                    out.append(Finding("doc-map", rel, f"missing:{path}", f"listed path does not exist: {path}", lineno))
    for f in repo.md_files:
        if not f.startswith("docs/") or f.startswith(("docs/phases/", "docs/knowledge/")):
            continue
        if f in listed or f in DOC_MAP_ALLOWLIST:
            continue
        out.append(Finding("doc-map", f, "unlisted", f"{f} is not listed in {rel}"))
    return out


REPO_PATH_RE = re.compile(r"^((?:kernel|shared|uefi-stub|scripts)/\S*)")


def clean_repo_path(token: str) -> str:
    token = token.split("::", 1)[0]
    token = re.sub(r":\d+(?:[-–]\d+)?(?:,\d+)*$", "", token)
    return token.rstrip(".,;:)")


def check_repo_paths(repo: Repo) -> list[Finding]:
    out = []
    for rel, allowed in repo.current_state_regions():
        for lineno, line in iter_prose_lines(repo.text(rel)):
            if allowed is not None and lineno not in allowed:
                continue
            for span in code_spans(line):
                m = REPO_PATH_RE.match(span)
                if not m:
                    continue
                path = clean_repo_path(m.group(1))
                if not path or is_path_placeholder(path):
                    continue
                if not repo.exists(path):
                    out.append(Finding("repo-paths", rel, path, f"path does not exist: {path}", lineno))
    return out


def documented_recipes(repo: Repo) -> set[str]:
    documented: set[str] = set()
    sources = [
        ("README.md", r"^## Build Commands", r"^## "),
        ("docs/project/developer-guide.md", r"^### 5\.1 ", r"^#{2,3} "),
    ]
    for rel, start, stop in sources:
        if rel not in repo.file_set:
            continue
        for _, line in section_body(repo.text(rel), start, stop):
            if line.lstrip().startswith("|"):
                for m in re.finditer(r"`just ([A-Za-z0-9_-]+)", line):
                    documented.add(m.group(1))
    return documented


def check_just_recipes(repo: Repo) -> list[Finding]:
    if "justfile" not in repo.file_set:
        raise Skip("no justfile")
    names, public = repo.justfile_recipes()
    out = []
    for rel, allowed in repo.current_state_regions():
        for lineno, line in iter_prose_lines(repo.text(rel)):
            if allowed is not None and lineno not in allowed:
                continue
            for span in code_spans(line):
                m = re.match(r"^just ([A-Za-z0-9_-]+)", span)
                if m and m.group(1) not in names:
                    out.append(Finding("just-recipes", rel, m.group(1), f"`just {m.group(1)}` is not a justfile recipe", lineno))
    documented = documented_recipes(repo)
    for name in sorted(public - documented - {"default"}):
        out.append(Finding("just-recipes", "justfile", f"undocumented:{name}",
                           f"public recipe `{name}` is missing from the README Build Commands and developer-guide §5.1 tables"))
    return out


def check_test_count(repo: Repo) -> list[Finding]:
    actual = 0
    for f in repo.files:
        if f.startswith("shared/src/") and f.endswith(".rs"):
            actual += len(re.findall(r"#\[test\]", repo.text(f)))
    claim_res = [
        re.compile(r"<!--\s*gen:test-count\s*-->\s*(\d+)"),
        re.compile(r"(?i)\bcurrent(?:ly)?\b[^\n]{0,60}?\b(\d{2,5}) (?:host(?:-side)? |unit )?tests\b"),
        re.compile(r"(?i)\bcurrent test distribution \((\d+) tests\)"),
    ]
    out = []
    for rel, allowed in repo.current_state_regions():
        if allowed is not None:
            continue  # phase docs record historical counts
        seen_lines = set()
        for lineno, line in iter_prose_lines(repo.text(rel)):
            for rx in claim_res:
                for m in rx.finditer(line):
                    claimed = int(m.group(1))
                    if claimed != actual and (lineno, claimed) not in seen_lines:
                        seen_lines.add((lineno, claimed))
                        out.append(Finding("test-count", rel, f"claimed:{claimed}",
                                           f"states {claimed} tests; shared/src has {actual} #[test] functions", lineno))
    return out


LOCK_NAME_RE = re.compile(r"\b[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+\b")
STATIC_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?static\s+([A-Z][A-Z0-9_]*)\s*:\s*(.*)$")


def code_mutex_statics(repo: Repo) -> dict[str, tuple[str, int]]:
    statics: dict[str, tuple[str, int]] = {}
    for f in repo.files:
        if not (f.startswith("kernel/src/") and f.endswith(".rs")):
            continue
        if posixpath.basename(f) == "tests.rs" or "/tests/" in f:
            continue
        lines = repo.text(f).splitlines()
        for i, line in enumerate(lines):
            if line.strip() == "#[cfg(test)]":
                nxt = next((ln for ln in lines[i + 1:] if ln.strip()), "")
                if re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{", nxt):
                    break  # an inline test module runs to the end of the file by convention
            m = STATIC_RE.match(line)
            if m and re.search(r"\bMutex\s*<", m.group(2)) and m.group(1) not in TEST_LOCKS:
                statics.setdefault(m.group(1), (f, i + 1))
    return statics


def check_lock_order(repo: Repo) -> list[Finding]:
    doc = "docs/kernel/deadlock-prevention.md"
    if doc not in repo.file_set:
        raise Skip(f"{doc} not found")
    statics = code_mutex_statics(repo)
    body = section_body(repo.text(doc), r"^### 3\.3 ", r"^### 3\.5 |^## 4\.")
    doc_locks: dict[str, int] = {}
    ranks: dict[str, int] = {}
    for lineno, cells in table_rows(body):
        name = None
        for c in cells:
            m = re.fullmatch(r"`([A-Z][A-Z0-9_]*)(?:\[[^\]]*\])?`", c)
            if m:
                name = m.group(1)
                break
        if not name:
            continue
        doc_locks.setdefault(name, lineno)
        if cells and cells[0].isdigit():
            ranks[name] = int(cells[0])
    out = []
    for name in sorted(set(statics) - set(doc_locks)):
        f, ln = statics[name]
        out.append(Finding("lock-order", doc, f"undocumented:{name}",
                           f"production lock {name} is not in §3.3/§3.4", detail=f"defined at {f}:{ln}"))
    for name in sorted(set(doc_locks) - set(statics)):
        out.append(Finding("lock-order", doc, f"stale:{name}",
                           f"§3.3/§3.4 lists {name}, which is not a Mutex static in kernel/src", doc_locks[name]))
    # CLAUDE.md lock ordering chain.
    claude = repo.text("CLAUDE.md").splitlines()
    chain_text = ""
    name_line: dict[str, int] = {}  # first CLAUDE.md line naming each lock in the chain
    for i, line in enumerate(claude):
        m = re.match(r"^Lock ordering[^:]*:\s*(.*)$", line)
        if m:
            chain = [(i + 1, m.group(1))]
            for j, nxt in enumerate(claude[i + 1:], i + 2):
                if not nxt.startswith("   ") or not nxt.strip():
                    break
                chain.append((j, nxt.strip()))
            for lineno, part in chain:
                for name in LOCK_NAME_RE.findall(part):
                    name_line.setdefault(name, lineno)
            chain_text = " ".join(part for _, part in chain)
            break
    if chain_text:
        groups = [LOCK_NAME_RE.findall(part) for part in re.split(r">(?![^{]*})", chain_text)]
        groups = [g for g in groups if g]
        for g in groups:
            for name in g:
                if name not in statics:
                    out.append(Finding("lock-order", "CLAUDE.md", f"unknown:{name}",
                                       f"lock ordering names {name}, which is not a Mutex static in kernel/src",
                                       name_line.get(name, 0)))
        for i, gi in enumerate(groups):
            for gj in groups[i + 1:]:
                for a in gi:
                    for b in gj:
                        if a in ranks and b in ranks and ranks[a] > ranks[b]:
                            out.append(Finding("lock-order", "CLAUDE.md", f"order:{a}>{b}",
                                               f"CLAUDE.md orders {a} before {b}, §3.3 ranks them {ranks[a]} and {ranks[b]}",
                                               name_line.get(b, 0)))
    # `Lock ordering` comment blocks in kernel code.
    for f in repo.files:
        if not (f.startswith("kernel/src/") and f.endswith(".rs")):
            continue
        lines = repo.text(f).splitlines()
        i = 0
        while i < len(lines):
            s = lines[i].strip()
            if s.startswith("//") and "lock ordering" in s.lower():
                block = [(i + 1, s)]
                j = i + 1
                while j < len(lines):
                    t = lines[j].strip()
                    if not t.startswith("//") or t.rstrip("/!").strip() == "":
                        break
                    block.append((j + 1, t))
                    j += 1
                reported = set()
                for ln, t in block:
                    for name in LOCK_NAME_RE.findall(t):
                        if name not in statics and name not in TEST_LOCKS and name not in reported:
                            reported.add(name)
                            out.append(Finding("lock-order", f, f"unknown:{name}",
                                               f"lock-ordering comment names {name}, which is not a Mutex static", ln))
                i = j
            else:
                i += 1
    return out


def check_milestone_status(repo: Repo) -> list[Finding]:
    merged = repo.merged_milestones()
    if not merged:
        raise Skip("no 'Phase N MK:' commits found on main's first-parent history")
    out = []
    phase_ms: dict[int, set[int]] = {}
    for phase, rel in repo.phase_docs():
        sections = repo.milestone_sections(rel)
        ms = set(sections)
        phase_ms[phase] = ms
        status_line, status = 0, ""
        for i, line in enumerate(repo.text(rel).splitlines(), 1):
            m = re.match(r"^\*\*Status:\*\*\s*(.*)$", line)
            if m:
                status_line, status = i, m.group(1).strip()
                break
        done = ms & set(merged)
        out.extend(status_findings(rel, f"status", status, ms, done, status_line))
        lines = repo.text(rel).splitlines()
        for num in sorted(done):
            lo, hi = sections[num]
            unchecked = [n for n in range(lo, hi + 1) if re.match(r"^\s*[-*] \[ \]", lines[n - 1])]
            if unchecked:
                out.append(Finding("milestone-status", rel, f"M{num}:unchecked",
                                   f"merged milestone M{num} still has {len(unchecked)} unchecked task(s)", unchecked[0]))
    latest = max(merged)
    if "README.md" in repo.file_set and latest not in milestone_tokens(repo.text("README.md")):
        out.append(Finding("milestone-status", "README.md", f"latest:M{latest}",
                           f"README status does not mention the latest merged milestone M{latest}"))
    plan = "docs/project/development-plan.md"
    if plan in repo.file_set:
        text = repo.text(plan)
        for lineno, cells in table_rows(section_body(text, r"^## 8\. ", r"^## ")):
            if len(cells) < 6 or not cells[0].isdigit():
                continue
            phase = int(cells[0])
            ms = phase_ms.get(phase) or {m for m, p in merged.items() if p == phase}
            done = ms & set(merged)
            out.extend(status_findings(plan, f"§8:phase-{phase}", cells[5], ms, done, lineno))
        covered: set[int] = set()
        for _, cells in table_rows(section_body(text, r"^### Velocity Summary", r"^#{2,3} ")):
            if len(cells) >= 5:
                covered |= milestone_tokens(cells[4])
        for num in sorted(set(merged) - covered):
            out.append(Finding("milestone-status", plan, f"§8.1:M{num}",
                               f"§8.1 Velocity Summary has no row covering merged milestone M{num}"))
    return out


def status_findings(rel: str, target: str, status: str, ms: set[int], done: set[int], lineno: int) -> list[Finding]:
    s = status.lower()
    if ms and done == ms and not s.startswith("complete"):
        return [Finding("milestone-status", rel, target,
                        f"all milestones ({fmt_ms(ms)}) are merged but status is '{status}'", lineno)]
    if done and done != ms and s.startswith("planned"):
        return [Finding("milestone-status", rel, target,
                        f"{fmt_ms(done)} merged but status is '{status}'", lineno)]
    if not done and s.startswith("complete"):
        return [Finding("milestone-status", rel, target, f"status is '{status}' but no milestone is merged", lineno)]
    return []


def fmt_ms(ms: set[int]) -> str:
    return ", ".join(f"M{m}" for m in sorted(ms))


def check_phase_count(repo: Repo) -> list[Finding]:
    plan = "docs/project/development-plan.md"
    if plan not in repo.file_set:
        raise Skip(f"{plan} not found")
    rows = [c for _, c in table_rows(section_body(repo.text(plan), r"^## 8\. ", r"^## ")) if c and c[0].isdigit()]
    actual = len(rows)
    sources = [
        (plan, r"\b(\d+) phases across\b"),
        ("README.md", r"\b(\d+) phases across\b"),
        (".claude/rules/07-milestone-numbering.md", r"\b(\d+) phases\b"),
        ("CLAUDE.md", r"\b(\d+) phases\b"),
    ]
    out = []
    for rel, rx in sources:
        if rel not in repo.file_set:
            continue
        for lineno, line in iter_prose_lines(repo.text(rel)):
            for m in re.finditer(rx, line):
                if int(m.group(1)) != actual:
                    out.append(Finding("phase-count", rel, f"claimed:{m.group(1)}",
                                       f"says {m.group(1)} phases; development-plan §8 lists {actual}", lineno))
    return out


def layout_block(repo: Repo) -> list[str]:
    lines = [l for _, l in section_body(repo.text("CLAUDE.md"), r"^## Workspace Layout", r"^## ")]
    return lines


def tree_entries(lines: list[str], start: str, stop: str) -> tuple[set[str], set[str]]:
    """(directory names, top-level module names) between two tree lines of the layout."""
    dirs: set[str] = set()
    mods: set[str] = set()
    inside = False
    collecting = False
    for line in lines:
        if start in line and "──" in line:
            inside = True
            continue
        if inside and stop in line and "──" in line:
            break
        if not inside:
            continue
        if "──" in line:
            collecting = False
            m = re.search(r"──\s+([A-Za-z0-9_.-]+)/", line)
            if m:
                dirs.add(m.group(1))
            if "(top-level)" in line:
                collecting = True
                text = line.split("(top-level)", 1)[1]
                mods.update(n[:-3] if n.endswith(".rs") else n for n in re.findall(r"[a-z_][a-z0-9_.]*", text))
        elif collecting:
            text = re.sub(r"^[│\s]+", "", line)
            mods.update(n[:-3] if n.endswith(".rs") else n for n in re.findall(r"[a-z_][a-z0-9_.]*", text))
    return dirs, mods


def check_layout(repo: Repo) -> list[Finding]:
    kdirs = {f.split("/")[2] for f in repo.files if f.startswith("kernel/src/") and f.count("/") >= 3}
    kmods = {posixpath.basename(f)[:-3] for f in repo.files if f.startswith("kernel/src/") and f.count("/") == 2 and f.endswith(".rs")}
    sdirs = {f.split("/")[2] for f in repo.files if f.startswith("shared/src/") and f.count("/") >= 3}
    smods = {posixpath.basename(f)[:-3] for f in repo.files if f.startswith("shared/src/") and f.count("/") == 2 and f.endswith(".rs")}
    out = []
    block = layout_block(repo)
    ldirs, lmods = tree_entries(block, "kernel/src/", "shared/src/")
    sd, sm = tree_entries(block, "shared/src/", "uefi-stub/")
    for label, actual, listed, fmt in (
        ("kernel dir", kdirs, ldirs, "kernel/src/{}/"),
        ("kernel module", kmods, lmods, "kernel/src/{}.rs"),
        ("shared dir", sdirs, sd, "shared/src/{}/"),
        ("shared module", smods, sm, "shared/src/{}.rs"),
    ):
        for name in sorted(actual - listed):
            out.append(Finding("layout", "CLAUDE.md", "missing:" + fmt.format(name),
                               f"Workspace Layout does not list {label} {fmt.format(name)}"))
        for name in sorted(listed - actual):
            out.append(Finding("layout", "CLAUDE.md", "stale:" + fmt.format(name),
                               f"Workspace Layout lists {fmt.format(name)}, which does not exist"))
    rule = ".claude/rules/05-file-placement.md"
    if rule in repo.file_set:
        rdirs = set(re.findall(r"^kernel/src/([a-z0-9_]+)/", repo.text(rule), re.M))
        for name in sorted(kdirs - rdirs):
            out.append(Finding("layout", rule, f"missing:kernel/src/{name}/", f"rule 05 does not list kernel/src/{name}/"))
        for name in sorted(rdirs - kdirs):
            out.append(Finding("layout", rule, f"stale:kernel/src/{name}/", f"rule 05 lists kernel/src/{name}/, which does not exist"))
    return out


def claude_table_names(repo: Repo, marker: str, rx: str) -> set[str]:
    names: set[str] = set()
    for _, line in section_body(repo.text("CLAUDE.md"), re.escape(marker), r"^\*\*|^## "):
        if line.lstrip().startswith("|"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            m = re.match(rx, cells[0]) if cells else None
            if m:
                names.add(m.group(1))
    return names


def layout_list(block: list[str], label: str) -> set[str] | None:
    names: set[str] = set()
    found = False
    for line in block:
        if "──" in line:
            if found:
                break
            m = re.search(r"──\s+" + re.escape(label) + r"/\s+(.*)$", line)
            if m:
                found = True
                names.update(re.findall(r"[a-z][a-z0-9-]*", m.group(1).split("(")[0]))
        elif found:
            names.update(re.findall(r"[a-z][a-z0-9-]*", re.sub(r"^[│\s]+", "", line).split("(")[0]))
    return names if found else None


def check_harness_tables(repo: Repo) -> list[Finding]:
    skills = set()
    for f in repo.files:
        m = re.match(r"^\.claude/skills/([^/]+)(?:/SKILL\.md)?$", f)
        if m:
            skills.add(m.group(1))
    agents = {m.group(1) for f in repo.files if (m := re.match(r"^\.claude/agents/([^/]+)\.md$", f))}
    out = []
    table_skills = claude_table_names(repo, "**Skills**", r"`/([a-z0-9-]+)")
    table_agents = claude_table_names(repo, "**Agents**", r"`([a-z0-9-]+)`")
    block = layout_block(repo)
    for kind, actual, listed, where in (
        ("skill", skills, table_skills, "skills-table"),
        ("agent", agents, table_agents, "agents-table"),
        ("skill", skills, layout_list(block, "skills"), "layout-skills"),
        ("agent", agents, layout_list(block, "agents"), "layout-agents"),
    ):
        if listed is None:
            continue
        for name in sorted(actual - listed):
            out.append(Finding("harness-tables", "CLAUDE.md", f"{where}-missing:{name}",
                               f"CLAUDE.md {where} omits {kind} {name}"))
        for name in sorted(listed - actual):
            out.append(Finding("harness-tables", "CLAUDE.md", f"{where}-stale:{name}",
                               f"CLAUDE.md {where} lists {kind} {name}, which is not in .claude/"))
    return out


def norm_section(name: str) -> str:
    s = name.lower().replace("&", " ")
    s = re.sub(r"[^a-z0-9 ]", " ", s)
    words = ["document" if w == "doc" else w for w in s.split() if w not in ("and", "the")]
    return " ".join(words)


def claude_sections(repo: Repo) -> dict[str, tuple[str, bool]]:
    """normalized '## ' heading -> (heading, is_pointer_stub)."""
    text = repo.text("CLAUDE.md").splitlines()
    out: dict[str, tuple[str, bool]] = {}
    for lineno, level, heading in repo.headings("CLAUDE.md"):
        if level != 2:
            continue
        body = []
        for line in text[lineno:]:
            if line.startswith("## "):
                break
            if line.strip() and line.strip() != "---":
                body.append(line)
        joined = " ".join(body)
        stub = len(body) <= 2 and bool(re.search(r"\b(lives in|moved to|are in|is in|see)\b", joined, re.I))
        out[norm_section(heading)] = (heading, stub)
    return out


def rule_titles(repo: Repo) -> dict[str, str]:
    """normalized heading -> rule file, for '# ' and '## ' headings in .claude/rules."""
    out: dict[str, str] = {}
    for f in repo.md_files:
        if f.startswith(".claude/rules/"):
            for _, level, heading in repo.headings(f):
                if level <= 2:
                    out.setdefault(norm_section(heading), f)
    return out


TITLE_WORD = r"[A-Z][A-Za-z0-9&/-]*"
# A run of Title Case section names joined by commas, "and", or a parenthetical
# aside: "Code Conventions (`.claude/rules/`) and Quality Gates".
TITLE_LIST = r"(" + TITLE_WORD + r"(?:\s*\([^)]*\)|\s*,\s*|\s+and\s+|\s+|" + TITLE_WORD + r")*)"
BEFORE_CLAUDE_RE = re.compile(
    r"((?:" + TITLE_WORD + r"\s+){0,6}" + TITLE_WORD + r")[\"'”*]*\s*\(?\s*(?:in|from)\s+`?CLAUDE\.md\b"
)
AFTER_CLAUDE_RE = re.compile(r"`?CLAUDE\.md`?\s*(:)?\s*[\"“]?" + TITLE_LIST)
# "2. Update: Workspace Layout, Key Technical Facts" under a heading that names CLAUDE.md.
LABELLED_ITEM_RE = re.compile(r"^\s*(?:[-*+]|\d+[.)])\s+(?:\*\*)?[A-Za-z][A-Za-z ]{0,30}?(?:\*\*)?:\s*" + TITLE_LIST)


def split_title_list(text: str, all_parts: bool) -> list[list[str]]:
    """Split a TITLE_LIST match into section-name word lists.

    "and" and parenthetical asides always separate names. Commas separate names
    only when all_parts is set (after a colon); otherwise the list ends at the
    first comma, because prose such as "CLAUDE.md Workspace Layout, then ..."
    continues with unrelated words.
    """
    chunks = text.split(",")
    if not all_parts:
        chunks = chunks[:1]
    out = []
    for chunk in chunks:
        for part in re.split(r"\([^)]*\)|\band\b", chunk):
            if part.split():
                out.append(part.split())
    return out


def section_candidates(line: str) -> list[tuple[str, list[str]]]:
    phrases = []
    for m in BEFORE_CLAUDE_RE.finditer(line):
        phrases.append(("before", m.group(1).split()))
    for m in AFTER_CLAUDE_RE.finditer(line):
        for words in split_title_list(m.group(2), all_parts=bool(m.group(1))):
            phrases.append(("after", words))
    return phrases


def labelled_item_candidates(line: str) -> list[tuple[str, list[str]]]:
    m = LABELLED_ITEM_RE.match(line)
    return [("after", words) for words in split_title_list(m.group(1), all_parts=True)] if m else []


def resolve_phrase(kind: str, words: list[str], sections: dict, rules: dict) -> tuple[str, str]:
    """Return (status, name): status in ok|stub|moved|missing|ignore."""
    n = len(words)
    options = [words[i:] for i in range(n)] if kind == "before" else [words[:j] for j in range(n, 0, -1)]
    for opt in options:
        name = " ".join(opt)
        key = norm_section(name)
        if key in sections:
            heading, stub = sections[key]
            return ("stub" if stub else "ok", heading)
    for opt in options:
        name = " ".join(opt)
        key = norm_section(name)
        if key in rules:
            return ("moved", f"{name}|{rules[key]}")
    name = " ".join(words)
    if len(words) < 2:
        return ("ignore", name)
    return ("missing", name)


def check_pointer_doctor(repo: Repo) -> list[Finding]:
    harness = [f for f in repo.md_files if f.startswith((".claude/agents/", ".claude/skills/", ".claude/rules/"))]
    sections = claude_sections(repo)
    rules = rule_titles(repo)
    skills = {m.group(1) for f in repo.files if (m := re.match(r"^\.claude/skills/([^/]+)", f))}
    agents = {m.group(1) for f in repo.files if (m := re.match(r"^\.claude/agents/([^/]+)\.md$", f))}
    out = []
    for rel in harness:
        text = repo.text(rel)
        # tools: frontmatter
        fm = re.match(r"^---\n(.*?)\n---", text, re.S)
        if fm:
            for i, line in enumerate(fm.group(1).splitlines(), 2):
                m = re.match(r"^tools:\s*(.*)$", line)
                if m:
                    for tool in [t.strip() for t in m.group(1).split(",") if t.strip()]:
                        if tool not in KNOWN_TOOLS and not tool.startswith("mcp__"):
                            out.append(Finding("pointer-doctor", rel, f"tool:{tool}", f"tools: lists unknown tool {tool}", i))
        under_claude_heading = False
        for lineno, line in iter_prose_lines(text):
            hm = HEADING_RE.match(line)
            if hm:
                under_claude_heading = "CLAUDE.md" in hm.group(2)
            phrases = section_candidates(line) if "CLAUDE.md" in line else []
            if under_claude_heading and not hm:
                phrases += labelled_item_candidates(line)
            line_keys: set[str] = set()
            for kind, words in phrases:
                status, name = resolve_phrase(kind, words, sections, rules)
                if status == "stub":
                    f = Finding("pointer-doctor", rel, f"claude-md:{name}",
                                f"points to CLAUDE.md '{name}', which is only a pointer stub now", lineno)
                elif status == "moved":
                    sec, where = name.split("|", 1)
                    f = Finding("pointer-doctor", rel, f"claude-md:{sec}",
                                f"points to CLAUDE.md '{sec}', which now lives in {where}", lineno)
                elif status == "missing":
                    f = Finding("pointer-doctor", rel, f"claude-md:{name}",
                                f"points to CLAUDE.md '{name}', which is not a section of CLAUDE.md", lineno)
                else:
                    continue
                if f.key not in line_keys:
                    line_keys.add(f.key)
                    out.append(f)
            for m in re.finditer(r"(?:\.claude/)?rules/(\d\d-[a-z0-9-]+\.md)", line):
                if f".claude/rules/{m.group(1)}" not in repo.file_set:
                    out.append(Finding("pointer-doctor", rel, f"rules:{m.group(1)}", f"rule file {m.group(1)} does not exist", lineno))
            for span in code_spans(line):
                if span.startswith("docs/"):
                    path = clean_repo_path(span.split()[0])
                    if not is_path_placeholder(path) and not repo.exists(path):
                        out.append(Finding("pointer-doctor", rel, f"path:{path}", f"path does not exist: {path}", lineno))
                m = re.match(r"^/([a-z][a-z0-9-]*)(?:\s|$)", span)
                if m and m.group(1) not in skills and m.group(1) not in BUILTIN_COMMANDS:
                    out.append(Finding("pointer-doctor", rel, f"skill:/{m.group(1)}", f"/{m.group(1)} is not a project skill or built-in command", lineno))
            masked_agents = re.finditer(r"`([a-z][a-z0-9-]*)`\s+(?:agent|subagent)\b|subagent_type:\s*`?([A-Za-z][A-Za-z0-9-]*)", line)
            for m in masked_agents:
                name = m.group(1) or m.group(2)
                if name not in agents and name not in BUILTIN_AGENTS:
                    out.append(Finding("pointer-doctor", rel, f"agent:{name}", f"agent {name} is not defined in .claude/agents", lineno))
    return out


def parse_frontmatter(text: str) -> dict[str, str] | None:
    m = re.match(r"^---\r?\n(.*?)\r?\n---\s*(?:\r?\n|$)", text, re.S)
    if not m:
        return None
    out = {}
    for line in m.group(1).splitlines():
        km = re.match(r"^([A-Za-z_][\w-]*):\s*(.*)$", line)
        if km:
            out[km.group(1)] = km.group(2).strip().strip("\"'")
    return out


def check_knowledge_hygiene(repo: Repo) -> list[Finding]:
    out = []
    for f in repo.md_files:
        if not f.startswith("docs/knowledge/"):
            continue
        base = posixpath.basename(f)
        if base in ("README.md", "_template.md"):
            continue
        if f.startswith("docs/knowledge/plans/"):
            out.append(Finding("knowledge-hygiene", f, "plans-not-empty",
                               "working plan present; distill it and remove it before the PR is ready"))
            continue
        if not KNOWLEDGE_NAME_RE.match(base):
            out.append(Finding("knowledge-hygiene", f, "name", "file name is not YYYY-MM-DD-initials-short-description.md"))
        fm = parse_frontmatter(repo.text(f))
        if fm is None:
            out.append(Finding("knowledge-hygiene", f, "frontmatter", "no YAML frontmatter"))
            continue
        for key in KNOWLEDGE_KEYS:
            if key not in fm:
                out.append(Finding("knowledge-hygiene", f, f"missing:{key}", f"frontmatter lacks '{key}'"))
        status = fm.get("status")
        allowed = KNOWLEDGE_STATUSES | KNOWLEDGE_STATUSES_EXTRA.get(f.split("/")[2], set())
        if status and status not in allowed:
            out.append(Finding("knowledge-hygiene", f, f"status:{status}",
                               f"status '{status}' is not one of {', '.join(sorted(allowed))}"))
    return out


CHECK_FUNCS = {
    "md-links": check_md_links,
    "section-refs": check_section_refs,
    "anchors": check_anchors,
    "wiki-links": check_wiki_links,
    "doc-map": check_doc_map,
    "repo-paths": check_repo_paths,
    "just-recipes": check_just_recipes,
    "test-count": check_test_count,
    "lock-order": check_lock_order,
    "milestone-status": check_milestone_status,
    "phase-count": check_phase_count,
    "layout": check_layout,
    "harness-tables": check_harness_tables,
    "pointer-doctor": check_pointer_doctor,
    "knowledge-hygiene": check_knowledge_hygiene,
}

# ---------------------------------------------------------------------------
# Baseline and reporting
# ---------------------------------------------------------------------------


def load_baseline(path: str) -> dict[str, dict]:
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except FileNotFoundError:
        return {}
    except (OSError, json.JSONDecodeError) as exc:
        sys.exit(f"docs-check: cannot read baseline {path}: {exc}")
    return {e["key"]: e for e in data.get("findings", [])}


def baseline_entry(f: Finding, old: dict | None) -> dict:
    """Baseline record for a finding; keeps an accepted-false-positive reason across rewrites."""
    entry = {"key": f.key, "check": f.check, "file": f.file, "target": f.target, "message": f.message}
    if f.count > 1:
        entry["count"] = f.count
    if old and old.get("reason"):
        entry["reason"] = old["reason"]
    return entry


def write_baseline(path: str, entries: dict[str, dict]) -> None:
    ordered = sorted(entries.values(), key=lambda e: (CHECK_ORDER.index(e["check"]) if e["check"] in CHECK_ORDER else 99, e["key"]))
    counts: dict[str, int] = {}
    for e in ordered:
        counts[e["check"]] = counts.get(e["check"], 0) + 1
    data = {
        "comment": ("Accepted docs drift. A finding is new when its key is missing here or it occurs on more "
                    "lines than 'count' (default 1). 'reason' marks an accepted false positive and survives "
                    "regeneration. Regenerate with: just docs-check --update-baseline"),
        "version": 1,
        "counts": {c: counts[c] for c in CHECK_ORDER if c in counts},
        "findings": ordered,
    }
    with open(path, "w", encoding="utf-8") as fh:
        json.dump(data, fh, indent=2, ensure_ascii=False)
        fh.write("\n")


def run_checks(repo: Repo, names: list[str]) -> tuple[list[Finding], dict[str, str]]:
    findings: dict[str, Finding] = {}
    skipped: dict[str, str] = {}
    for name in names:
        try:
            result = CHECK_FUNCS[name](repo)
        except Skip as exc:
            skipped[name] = str(exc)
            continue
        for f in result:
            first = findings.setdefault(f.key, f)
            if first is not f and f.line and f.line != first.line and f.line not in first.also:
                first.also = first.also + (f.line,)
    ordered = sorted(findings.values(), key=lambda f: (CHECK_ORDER.index(f.check), f.file, f.line, f.target))
    return ordered, skipped


@dataclass
class Comparison:
    new_keys: set[str]                   # key missing from the baseline, or more occurrences than baselined
    grown: dict[str, int]                # key -> baselined count, for keys whose count increased
    accepted: dict[str, str]             # key -> reason, for baselined false positives
    resolved: list[str]                  # baselined keys (of checks that ran) that no longer occur
    reduced: dict[str, tuple[int, int]]  # key -> (baselined count, current count), for keys that shrank


def compare(findings: list[Finding], baseline: dict[str, dict], ran: set[str]) -> Comparison:
    current = {f.key: f for f in findings}
    cmp = Comparison(set(), {}, {}, [], {})
    for key, f in current.items():
        entry = baseline.get(key)
        if entry is None:
            cmp.new_keys.add(key)
            continue
        base_count = int(entry.get("count", 1))
        if f.count > base_count:
            cmp.new_keys.add(key)
            cmp.grown[key] = base_count
        elif f.count < base_count:
            cmp.reduced[key] = (base_count, f.count)
        if entry.get("reason"):
            cmp.accepted[key] = entry["reason"]
    cmp.resolved = sorted(k for k, e in baseline.items() if e.get("check") in ran and k not in current)
    return cmp


def describe(f: Finding, cmp: Comparison) -> str:
    text = f.text()
    if f.key in cmp.grown:
        text += f" [{f.count} occurrences, baseline {cmp.grown[f.key]}]"
    if f.key in cmp.accepted:
        text += f" [accepted: {cmp.accepted[f.key]}]"
    return text


def prune_notes(cmp: Comparison, limit: int = 50) -> list[str]:
    notes = [f"  - {key}" for key in cmp.resolved]
    notes += [f"  - {key} ({b} -> {c} occurrences)" for key, (b, c) in sorted(cmp.reduced.items())]
    if len(notes) > limit:
        notes = notes[:limit] + [f"  ... and {len(notes) - limit} more"]
    return notes


def render_text(findings, cmp, skipped, names, show_all, baseline_rel) -> str:
    out = []
    total = len(findings)
    new = [f for f in findings if f.key in cmp.new_keys]
    out.append(f"docs-check: {total} findings across {len(names) - len(skipped)} checks - "
               f"{len(new)} new, {total - len(new)} baselined ({len(cmp.accepted)} accepted false positives), "
               f"{len(cmp.resolved)} resolved (baseline {baseline_rel})")
    out.append("")
    out.append(f"  {'check':<18} {'total':>5} {'new':>5}")
    for name in names:
        if name in skipped:
            out.append(f"  {name:<18} {'skip':>5}        {skipped[name]}")
            continue
        t = sum(1 for f in findings if f.check == name)
        n = sum(1 for f in new if f.check == name)
        out.append(f"  {name:<18} {t:>5} {n:>5}")
    shown = findings if show_all else new
    if shown:
        out.append("")
        out.append("All findings ('+' = new since baseline, '~' = accepted false positive):" if show_all
                   else "New drift since baseline:")
        current = None
        for f in shown:
            if f.check != current:
                current = f.check
                out.append(f"\n[{current}]")
            mark = "+" if f.key in cmp.new_keys else "~" if f.key in cmp.accepted else " "
            out.append(f" {mark} {f.location()}: {describe(f, cmp)}")
    elif not show_all:
        out.append("")
        out.append("No new drift since baseline.")
    notes = prune_notes(cmp)
    if notes:
        out.append("")
        out.append(f"{len(cmp.resolved) + len(cmp.reduced)} baselined finding(s) no longer occur or occur less often; "
                   "prune them with `just docs-check --update-baseline`:")
        out.extend(notes)
    return "\n".join(out)


def render_markdown(findings, cmp, skipped, names) -> str:
    new = [f for f in findings if f.key in cmp.new_keys]
    out = ["## Docs drift check", ""]
    out.append(f"**{len(new)} new** finding(s) since baseline; {len(findings)} total "
               f"({len(cmp.accepted)} accepted false positives), {len(cmp.resolved)} resolved. "
               "Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.")
    out.append("")
    out.append("| Check | Total | New |")
    out.append("|---|---:|---:|")
    for name in names:
        if name in skipped:
            out.append(f"| {name} | skipped | {skipped[name]} |")
        else:
            t = sum(1 for f in findings if f.check == name)
            n = sum(1 for f in new if f.check == name)
            out.append(f"| {name} | {t} | {n} |")
    if new:
        out.append("")
        out.append("### New findings")
        out.append("")
        for f in new[:100]:
            out.append(f"- `{f.check}` {f.location()}: {describe(f, cmp)}")
        if len(new) > 100:
            out.append(f"- ... and {len(new) - 100} more")
    if cmp.resolved or cmp.reduced:
        out.append("")
        out.append(f"{len(cmp.resolved) + len(cmp.reduced)} baselined finding(s) no longer occur or occur less often "
                   "(run `just docs-check --update-baseline`).")
    return "\n".join(out) + "\n"


def repo_root() -> str:
    here = os.path.dirname(os.path.abspath(__file__))
    r = subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=here, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit("docs-check: not inside a git repository")
    return r.stdout.strip()


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--all", action="store_true", help="list every finding, not only new ones")
    ap.add_argument("--json", action="store_true", help="print JSON instead of text")
    ap.add_argument("--markdown", action="store_true", help="print a Markdown summary (for $GITHUB_STEP_SUMMARY)")
    ap.add_argument("--check", default="", help="comma-separated checks to run (default: all)")
    ap.add_argument("--baseline", default=None, help=f"baseline file (default: {BASELINE_REL})")
    ap.add_argument("--update-baseline", action="store_true",
                    help="rewrite the baseline from the current findings (counts included; 'reason' on accepted false positives is kept)")
    ap.add_argument("--list-checks", action="store_true", help="list available checks")
    args = ap.parse_args(argv)

    if args.list_checks:
        for name in CHECK_ORDER:
            print(f"{name:<18} {CHECK_DESCRIPTIONS[name]}")
        return 0

    names = CHECK_ORDER
    if args.check:
        names = [n.strip() for n in args.check.split(",") if n.strip()]
        unknown = [n for n in names if n not in CHECK_FUNCS]
        if unknown:
            ap.error(f"unknown check(s): {', '.join(unknown)} (see --list-checks)")
        names = [n for n in CHECK_ORDER if n in names]

    root = repo_root()
    baseline_path = args.baseline or os.path.join(root, BASELINE_REL)
    baseline_rel = os.path.relpath(baseline_path, root)
    repo = Repo(root)
    findings, skipped = run_checks(repo, names)
    ran = set(names) - set(skipped)
    baseline = load_baseline(baseline_path)

    if args.update_baseline:
        entries = {k: e for k, e in baseline.items() if e.get("check") not in ran}
        for f in findings:
            entries[f.key] = baseline_entry(f, baseline.get(f.key))
        write_baseline(baseline_path, entries)
        print(f"docs-check: wrote {len(entries)} findings to {baseline_rel}"
              + (f" (skipped: {', '.join(sorted(skipped))})" if skipped else ""))
        return 0

    cmp = compare(findings, baseline, ran)

    if args.json:
        data = {
            "baseline": baseline_rel,
            "summary": {
                "total": len(findings),
                "new": len(cmp.new_keys),
                "baselined": len(findings) - len(cmp.new_keys),
                "accepted": len(cmp.accepted),
                "resolved": len(cmp.resolved),
                "reduced": len(cmp.reduced),
            },
            "checks": {
                n: ({"skipped": skipped[n]} if n in skipped else {
                    "total": sum(1 for f in findings if f.check == n),
                    "new": sum(1 for f in findings if f.check == n and f.key in cmp.new_keys),
                }) for n in names
            },
            "findings": [
                {"key": f.key, "check": f.check, "file": f.file, "line": f.line, "also": list(f.also),
                 "count": f.count,
                 "baseline_count": (None if f.key not in baseline else int(baseline[f.key].get("count", 1))),
                 "target": f.target, "message": f.message, "detail": f.detail,
                 "new": f.key in cmp.new_keys, "accepted": cmp.accepted.get(f.key)}
                for f in findings if args.all or f.key in cmp.new_keys
            ],
            "resolved": cmp.resolved,
            "reduced": {k: {"baseline": b, "current": c} for k, (b, c) in sorted(cmp.reduced.items())},
        }
        print(json.dumps(data, indent=2, ensure_ascii=False))
    elif args.markdown:
        sys.stdout.write(render_markdown(findings, cmp, skipped, names))
    else:
        print(render_text(findings, cmp, skipped, names, args.all, baseline_rel))
    return 1 if cmp.new_keys else 0


if __name__ == "__main__":
    sys.exit(main())
