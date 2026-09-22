#!/usr/bin/env python3
"""PreToolUse guard for the Bash and Monitor tools. It works out which git
and gh commands a shell command would run and stops the ones that must go
through the user.

Claude Code's permission rules match command text. Git accepts unambiguous
prefixes of long options (`--exe` for `--exec`) and clustered short options
(`-kx`), and the shell removes quotes (`ma'i'n`), so text rules miss forms
that git treats as the dangerous one. This hook parses the command the way a
shell does (quotes, `$(...)`, backticks, heredocs, `case`, `sh -c`, `eval`,
`read`, git aliases, script files), reads git options with git's own
abbreviation and cluster rules, and resolves each push destination the way
git does (bare name, heads/X, refs/heads/X, HEAD, implicit push via
push.default and the branch upstream).

Decisions (the strictest one wins):
  deny  a push that updates or deletes main; plain force pushes (--force, -f,
        +refspec); --all, --branches, --mirror; matching pushes (":" or
        push.default=matching); gh pr merge --admin
  ask   git push: deleting any other remote branch; --prune; --force-with-lease
          outside claude/*; --receive-pack/--exec; inline -c or --config-env;
          remote.<name>.push or .mirror in config; arguments supplied by
          xargs/find/parallel; a destination it cannot resolve; commits under
          .github/ that are not on the remote (CI runs them with repository
          secrets); GIT_*, HOME, XDG_CONFIG_HOME or PATH set for the push; an
          earlier command in the same text that changes git config, branches
          or the checked-out branch
        other git: rebase --exec (any prefix, or x in a short cluster); fetch
          or pull --upload-pack (any prefix); fetch or pull that force-updates
          local branches; checkout/switch --force, --merge, --conflict,
          --discard-changes, -B, -C; add --force; commit --file or --template
          outside the repository or from a pipe; branch -D, -f, -M, -C;
          worktree add -B; worktree remove --force; --output
        gh: issue/pr create, comment, edit or review against another
          repository, with a body file outside the repository or from a pipe,
          or mentioning @claude; gh api writes other than routine comments,
          replies, reviews and reactions on this repository; GraphQL
          mutations that merge, move refs or change repository settings;
          gh pr merge; gh alias changes; gh commands that are aliases or
          extensions
        shell: a command name, eval string, sh -c string or script path that
          comes from an expansion the guard cannot resolve; a shell or
          interpreter reading its program from a pipe; an awk program that
          runs shell commands; gh run download into .git/, .claude/,
          .github/, .cargo/ or outside the repository
  none  everything else; the permission rules and auto mode decide.

This is defence in depth, not a security boundary: code the guard does not
read (a Python script, a build step, a justfile recipe) can still run git or
gh. The ruleset on main is the boundary. The guard fails closed: an
unreadable payload or an internal error produces an ask.
"""

import json
import os
import re
import shlex
import subprocess
import sys

PROTECTED_BRANCH = "main"
AGENT_PREFIX = "claude/"
HOME_REPO = "juslee/aios"
MAX_DEPTH = 8
FILE_LIMIT = 256 * 1024

SHELLS = {"sh", "bash", "zsh", "dash", "ksh", "mksh", "fish", "busybox"}
INTERPRETERS = {
    "awk", "gawk", "nawk", "mawk", "perl", "python", "python3", "ruby",
    "node", "osascript", "php", "lua", "tclsh", "expect",
}
# Commands that never execute their arguments.
NON_EXEC = {
    "echo", "printf", "grep", "egrep", "fgrep", "rg", "ag", "ack", "man",
    "which", "type", "whereis", "whatis", "apropos", "info", "cat", "head",
    "tail", "less", "more", "wc", "ls", "file", "stat", "test", "[", "[[",
    "true", "false", ":", "jq", "diff", "cmp", "basename", "dirname",
}
RESERVED = {"{", "}", "!", "if", "then", "else", "elif", "fi", "do", "done",
            "while", "until", "time", "in", "case", "esac", "function"}
ARG_WRAPPERS = {"xargs", "parallel", "find"}
# Commands that run their arguments as a command line.
WRAPPERS = ARG_WRAPPERS | {"env", "command", "builtin", "exec", "nohup", "nice",
                           "timeout", "gtimeout", "time", "stdbuf", "sudo", "doas",
                           "caffeinate", "script", "unbuffer", "chronic", "flock",
                           "rustup", "arch"}
ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?\+?=")
EXPANSION_START = re.compile(r"[A-Za-z_{0-9@*#?$!-]")
INLINE_GIT_PUSH = re.compile(r"\bgit\b.*\bpush\b", re.S)
# Text that suggests a command string or script is about git or gh.
MENTIONS_GIT = re.compile(r"git|push|\bgh\b")

# Environment variables that change which repository, config or binary a
# git or gh command uses.
HARMLESS_GIT_ENV = {
    "GIT_PAGER", "GIT_TERMINAL_PROMPT", "GIT_OPTIONAL_LOCKS", "GIT_EDITOR",
    "GIT_SEQUENCE_EDITOR", "GIT_MERGE_AUTOEDIT", "GIT_MERGE_VERBOSITY",
    "GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL", "GIT_COMMITTER_DATE",
    "GIT_PROGRESS_DELAY", "GIT_FLUSH", "GIT_REDACT_COOKIES", "GIT_ADVICE",
}
GH_RISKY_ENV = {"GH_REPO", "GH_HOST", "GH_CONFIG_DIR"}
SHARED_RISKY_ENV = {"HOME", "XDG_CONFIG_HOME", "PATH"}

# Builtins never shadowed by aliases; anything else gets an alias lookup.
GIT_BUILTINS = set("""
add am annotate apply archive backfill bisect blame branch bugreport bundle
cat-file check-attr check-ignore check-mailmap check-ref-format checkout
checkout-index cherry cherry-pick citool clean clone column commit
commit-graph commit-tree config count-objects credential describe diff
diff-files diff-index diff-tree difftool fast-export fast-import fetch
fetch-pack filter-branch fmt-merge-msg for-each-ref for-each-repo
format-patch fsck gc get-tar-commit-id grep gui hash-object help
hook index-pack init instaweb interpret-trailers log ls-files ls-remote
ls-tree mailinfo mailsplit maintenance merge merge-base merge-file
merge-index merge-tree mergetool mktag mktree multi-pack-index mv
name-rev notes pack-objects pack-refs patch-id prune prune-packed pull
range-diff read-tree rebase reflog refs remote repack replace replay
request-pull rerere reset restore rev-list rev-parse revert rm
send-email shortlog show show-branch show-index show-ref sparse-checkout
stash status stripspace submodule switch symbolic-ref tag unpack-file
unpack-objects update-index update-ref update-server-info var verify-commit
verify-pack verify-tag version whatchanged worktree write-tree lfs
""".split())

# Git subcommands that neither change refs, config nor the working tree.
GIT_READ_ONLY = set("""
blame cat-file check-attr check-ignore check-mailmap check-ref-format cherry
count-objects describe diff diff-files diff-index diff-tree fetch
for-each-ref fsck grep help log ls-files ls-remote ls-tree merge-base
name-rev range-diff rev-list rev-parse shortlog show show-branch show-ref
status var verify-commit verify-pack verify-tag version whatchanged
""".split())
# Git subcommands that can change which branch is checked out.
GIT_HEAD_CHANGERS = {"checkout", "switch", "symbolic-ref", "update-ref",
                     "bisect", "rebase", "worktree", "branch"}

PUSH_LONG_OPTS = [
    "all", "branches", "mirror", "delete", "tags", "follow-tags", "dry-run",
    "porcelain", "force", "force-with-lease", "force-if-includes", "repo",
    "set-upstream", "thin", "quiet", "verbose", "progress",
    "recurse-submodules", "verify", "atomic", "push-option", "receive-pack",
    "exec", "prune", "signed", "ipv4", "ipv6",
]
PUSH_OPTS_WITH_ARG = {"repo", "push-option", "receive-pack", "exec",
                      "recurse-submodules"}
GIT_GLOBAL_WITH_ARG = {"--git-dir", "--work-tree", "--namespace",
                       "--super-prefix", "--config-env", "--attr-source"}

# gh top-level commands (plus the default `co` alias). Anything else is a
# user alias or an extension.
GH_COMMANDS = set("""
accessibility agent-task alias api attestation auth browse cache co codespace
completion config copilot extension gist gpg-key help issue label licenses
org pr preview project release repo ruleset run search secret ssh-key status
variable version workflow
""".split())
# gh commands that publish text to GitHub, with the shorthand flags that take
# a value. pflag lets boolean shorthands share a cluster with one value flag.
GH_PUBLISH = {
    ("issue", "create"): "abFlmpRtT",
    ("issue", "comment"): "bFR",
    ("issue", "edit"): "bFmRt",
    ("pr", "create"): "abBFHlmprRtT",
    ("pr", "comment"): "bFR",
    ("pr", "edit"): "bBFmRt",
    ("pr", "review"): "bFR",
}
API_VALUE_LONGS = {"method", "field", "raw-field", "header", "input", "jq",
                   "template", "preview", "hostname", "cache"}
API_VALUE_SHORTS = {"X": "method", "F": "field", "f": "raw-field", "H": "header",
                    "q": "jq", "t": "template", "p": "preview"}
# awk statements that run a shell command.
AWK_EXEC = re.compile(r"\bsystem\s*\(|\|\s*getline|\|&|\bprintf?\b[^;{}]*\|\s*[\"a-zA-Z_$(]")
AWK_NAMES = {"awk", "gawk", "nawk", "mawk"}
# Directories whose contents run as code or configure the harness.
PROTECTED_DIRS = (".git", ".claude", ".github", ".cargo")
GQL_DANGEROUS = re.compile(
    r"mergePullRequest|PullRequestAutoMerge|mergeBranch|updateRef|deleteRef|"
    r"createRef|createCommitOnBranch|BranchProtectionRule|RepositoryRuleset|"
    r"updateRepository|deleteRepository|archiveRepository|transferRepository|"
    r"updatePullRequestBranch|dismissPullRequestReview", re.I)


class ParseError(Exception):
    pass


# --------------------------------------------------------------------------
# Shell parsing
# --------------------------------------------------------------------------

class Command:
    __slots__ = ("words", "expanded", "heredocs", "stdin_files", "piped_in", "src")

    def __init__(self):
        self.words = []
        self.expanded = []  # per word: holds a $ or ` expansion outside single quotes
        self.heredocs = []
        self.stdin_files = []
        self.piped_in = False
        self.src = ""  # the command's own source text


class ShellParser:
    """Splits shell text into simple commands (lists of words with quoting
    removed). Command substitutions, process substitutions and backticks are
    parsed recursively; their commands are emitted before the command that
    contains them. Heredoc and here-string bodies are attached to their
    command as data, as are `< file` redirections and whether the command
    reads a pipe. `case` patterns are skipped; their bodies are commands."""

    def __init__(self, text, start, out, nested, depth):
        if depth > MAX_DEPTH:
            raise ParseError("nesting too deep")
        self.s = text
        self.n = len(text)
        self.i = start
        self.out = out
        self.nested = nested
        self.depth = depth
        self.parens = 0
        self.cmd = Command()
        self.word = None
        self.quoted = False
        self.word_expanded = False
        self.skip_next = False
        self.stdin_next = False
        self.herestring_next = False
        self.pending = []
        self.brace = False
        self.case = []  # per open `case`: "subject", "pattern" or "body"
        self.start = start

    def peek(self, k=0):
        j = self.i + k
        return self.s[j] if j < self.n else ""

    def add(self, text, quoted=False):
        self.word = (self.word or "") + text
        self.quoted = self.quoted or quoted

    def end_word(self):
        if self.word is None:
            return
        word, self.word = self.word, None
        brace, self.brace = self.brace, False
        quoted, self.quoted = self.quoted, False
        expanded, self.word_expanded = self.word_expanded, False
        if self.skip_next:
            self.skip_next = False
        elif self.stdin_next:
            self.stdin_next = False
            self.cmd.stdin_files.append(word)
        elif self.herestring_next:
            self.herestring_next = False
            self.cmd.heredocs.append(word)
        elif not self.case_keyword(word, quoted):
            words = brace_expand(word) if brace else [word]
            self.cmd.words.extend(words)
            self.cmd.expanded.extend([expanded] * len(words))

    def case_keyword(self, word, quoted):
        """Tracks `case WORD in PATTERN) BODY ;; ... esac`. Returns True when
        the word belongs to the case syntax rather than to a command."""
        state = self.case[-1] if self.case else None
        at_start = all(w in RESERVED for w in self.cmd.words)
        if state == "subject":
            if word == "in" and not quoted:
                self.case[-1] = "pattern"
            return True
        if state == "pattern":
            if word == "esac" and not quoted:
                self.case.pop()
            return True
        if quoted or not at_start:
            return False
        if word == "case":
            self.case.append("subject")
            return True
        if state == "body" and word == "esac":
            self.case.pop()
            return True
        return False

    def end_command(self):
        self.end_word()
        self.cmd.src = self.s[self.start:self.i]
        self.start = self.i
        waiting = any(p[2] is self.cmd for p in self.pending)
        if self.cmd.words or self.cmd.heredocs or self.cmd.stdin_files or waiting:
            self.out.append(self.cmd)
        self.cmd = Command()

    def read_heredocs(self):
        while self.pending:
            delim, strip_tabs, cmd = self.pending.pop(0)
            body = []
            while self.i < self.n:
                j = self.s.find("\n", self.i)
                line = self.s[self.i:] if j < 0 else self.s[self.i:j]
                self.i = self.n if j < 0 else j + 1
                if (line.lstrip("\t") if strip_tabs else line) == delim:
                    break
                body.append(line)
            cmd.heredocs.append("\n".join(body))

    def read_heredoc_delim(self):
        s = self.s
        while self.i < self.n and s[self.i] in " \t":
            self.i += 1
        buf = []
        while self.i < self.n and s[self.i] not in " \t\n;&|()<>":
            c = s[self.i]
            if c in "'\"":
                j = s.find(c, self.i + 1)
                if j < 0:
                    raise ParseError("unterminated heredoc delimiter")
                buf.append(s[self.i + 1:j])
                self.i = j + 1
            elif c == "\\":
                buf.append(s[self.i + 1:self.i + 2])
                self.i += 2
            else:
                buf.append(c)
                self.i += 1
        if not buf:
            raise ParseError("missing heredoc delimiter")
        return "".join(buf)

    def read_single(self):
        j = self.s.find("'", self.i + 1)
        if j < 0:
            raise ParseError("unterminated single quote")
        text = self.s[self.i + 1:j]
        self.i = j + 1
        return text

    def read_double(self):
        s = self.s
        i = self.i + 1
        buf = []
        while i < self.n:
            c = s[i]
            if c == '"':
                self.i = i + 1
                return "".join(buf)
            if c == "\\" and i + 1 < self.n and s[i + 1] in '$`"\\\n':
                if s[i + 1] != "\n":
                    buf.append(s[i + 1])
                i += 2
            elif c == "$" and i + 1 < self.n and s[i + 1] == "(":
                self.i = i
                buf.append(self.read_substitution(2))
                self.word_expanded = True
                i = self.i
            elif c == "`":
                self.i = i
                buf.append(self.read_backtick())
                self.word_expanded = True
                i = self.i
            else:
                if c == "$" and EXPANSION_START.match(s, i + 1):
                    self.word_expanded = True
                buf.append(c)
                i += 1
        raise ParseError("unterminated double quote")

    def read_ansi_c(self):
        s = self.s
        i = self.i + 2
        buf = []
        simple = {"n": "\n", "t": "\t", "r": "\r", "a": "\a", "b": "\b",
                  "e": "\x1b", "E": "\x1b", "f": "\f", "v": "\v",
                  "\\": "\\", "'": "'", '"': '"', "?": "?"}
        while i < self.n:
            c = s[i]
            if c == "'":
                self.i = i + 1
                return "".join(buf)
            if c != "\\" or i + 1 >= self.n:
                buf.append(c)
                i += 1
                continue
            esc = s[i + 1]
            i += 2
            if esc in simple:
                buf.append(simple[esc])
                continue
            pattern = {"x": r"[0-9A-Fa-f]{1,2}", "u": r"[0-9A-Fa-f]{1,4}",
                       "U": r"[0-9A-Fa-f]{1,8}"}.get(esc)
            if pattern:
                m = re.match(pattern, s[i:])
                if m:
                    try:
                        buf.append(chr(int(m.group(), 16)))
                    except (ValueError, OverflowError):
                        pass
                    i += len(m.group())
                else:
                    buf.append("\\" + esc)
            elif esc in "01234567":
                m = re.match(r"[0-7]{0,2}", s[i:])
                buf.append(chr(int(esc + m.group(), 8) & 0xFF))
                i += len(m.group())
            elif esc == "c" and i < self.n:
                buf.append(chr(ord(s[i]) & 0x1F))
                i += 1
            else:
                buf.append("\\" + esc)
        raise ParseError("unterminated $'...'")

    def read_substitution(self, skip):
        """At `$(`, `<(` or `>(`: parse the body as nested commands."""
        if skip == 2 and self.s.startswith("$((", self.i):
            depth = 0
            j = self.i + 1
            while j < self.n:
                if self.s[j] == "(":
                    depth += 1
                elif self.s[j] == ")":
                    depth -= 1
                    if depth == 0:
                        self.i = j + 1
                        return "$((...))"
                j += 1
            raise ParseError("unterminated $((")
        sub = ShellParser(self.s, self.i + skip, self.out, True, self.depth + 1)
        self.i = sub.parse() + 1
        return "$(...)"

    def read_backtick(self):
        s = self.s
        i = self.i + 1
        buf = []
        while i < self.n:
            c = s[i]
            if c == "\\" and i + 1 < self.n and s[i + 1] in "`$\\":
                buf.append(s[i + 1])
                i += 2
            elif c == "`":
                inner = "".join(buf)
                ShellParser(inner, 0, self.out, False, self.depth + 1).parse()
                self.i = i + 1
                return "`...`"
            else:
                buf.append(c)
                i += 1
        raise ParseError("unterminated backtick")

    def read_redirect(self):
        """At `<`, `>` or `&>`: consume the operator. The next word is its
        target: kept as a stdin file for `<` and `0<`, skipped otherwise,
        unless it is a process substitution."""
        fd = None
        if self.word is not None and self.word.isdigit():
            fd, self.word = self.word, None
        else:
            self.end_word()
        c = self.peek()
        if c in "<>" and self.peek(1) == "(":
            self.add(self.read_substitution(2))
            self.end_word()
            return
        if c == "&":
            self.i += 2
            if self.peek() == ">":
                self.i += 1
        else:
            self.i += 1
            if self.peek() in ">&|" or (c == "<" and self.peek() == ">"):
                self.i += 1
            elif c == "<" and fd in (None, "0"):
                self.stdin_next = True
                return
        self.skip_next = True

    def parse(self):
        s = self.s
        while self.i < self.n:
            c = s[self.i]
            if c in "|()" and self.case and self.case[-1] == "pattern":
                self.end_word()  # may be `esac`, which closes the case
            state = self.case[-1] if self.case else None
            if c in " \t\r":
                self.end_word()
                self.i += 1
            elif c == "\n":
                self.end_command()
                self.i += 1
                self.read_heredocs()
            elif c == "\\":
                if self.peek(1) == "\n":
                    self.i += 2
                else:
                    self.add(self.peek(1), quoted=True)
                    self.i += 2
            elif c == "'":
                self.add(self.read_single(), quoted=True)
            elif c == '"':
                self.add(self.read_double(), quoted=True)
            elif c == "$" and self.peek(1) == "'":
                self.add(self.read_ansi_c(), quoted=True)
            elif c == "$" and self.peek(1) == '"':
                self.i += 1  # $"..." is a translatable double-quoted string
            elif c == "$" and self.peek(1) == "(":
                self.add(self.read_substitution(2))
                self.word_expanded = True
            elif c == "`":
                self.add(self.read_backtick())
                self.word_expanded = True
            elif c == "#" and self.word is None:
                j = s.find("\n", self.i)
                self.i = self.n if j < 0 else j
            elif s.startswith("<<<", self.i):
                self.end_word()
                self.i += 3
                self.herestring_next = True
            elif s.startswith("<<", self.i):
                self.end_word()
                self.i += 2
                strip_tabs = self.peek() == "-"
                if strip_tabs:
                    self.i += 1
                self.pending.append((self.read_heredoc_delim(), strip_tabs, self.cmd))
            elif c in "<>" or (c == "&" and self.peek(1) == ">"):
                self.read_redirect()
            elif state == "pattern" and c in "|(":
                self.i += 1  # pattern alternatives and the optional "("
            elif state == "pattern" and c == ")":
                self.case[-1] = "body"
                self.i += 1
            elif c in ";&|":
                two = s[self.i:self.i + 2]
                if state == "body" and two in (";;", ";&"):
                    self.end_command()
                    self.case[-1] = "pattern"
                    self.i += 2
                    continue
                self.end_command()
                if c == "|" and two != "||":
                    self.cmd.piped_in = True
                self.i += 2 if two in ("&&", "||", ";;", "|&", ";&") else 1
            elif c == "(":
                self.end_command()
                self.parens += 1
                self.i += 1
            elif c == ")":
                self.end_command()
                if self.parens == 0 and self.nested:
                    return self.i
                self.parens = max(0, self.parens - 1)
                self.i += 1
            else:
                if c == "{":
                    self.brace = True
                elif c == "$" and EXPANSION_START.match(s, self.i + 1):
                    self.word_expanded = True
                self.add(c)
                self.i += 1
        if self.nested:
            raise ParseError("unterminated $(")
        self.end_command()
        return self.i


def brace_expand(word, limit=64):
    """Bash brace expansion for comma lists (`pu{sh,ll}`), so that
    `git {push,origin,main}` is seen as `git push origin main`."""
    depth = 0
    start = None
    commas = []
    for i, c in enumerate(word):
        if c == "{":
            if depth == 0:
                start, commas = i, []
            depth += 1
        elif c == "}" and depth:
            depth -= 1
            if depth == 0 and commas:
                prefix, suffix = word[:start], word[i + 1:]
                bounds = [start] + commas + [i]
                out = []
                for a, b in zip(bounds, bounds[1:]):
                    for tail in brace_expand(suffix, limit):
                        out.extend(brace_expand(prefix + word[a + 1:b] + tail, limit))
                        if len(out) >= limit:
                            return out[:limit]
                return out
        elif c == "," and depth == 1:
            commas.append(i)
    return [word]


def parse_commands(text, depth=0):
    out = []
    ShellParser(text, 0, out, False, depth).parse()
    return out


# --------------------------------------------------------------------------
# Option scanning
# --------------------------------------------------------------------------

def scan_git_options(args, longs, shorts="", takes_arg="", optional_arg=""):
    """Finds options the way git's parse-options reads them, up to `--`.

    A long option matches every name in `longs` it is a non-empty prefix of:
    git accepts unambiguous prefixes and rejects ambiguous ones, so matching
    all of them never misses a form git runs. `longs` maps each name to
    whether it takes a value. A short option can sit anywhere in a cluster
    (`-qf`); a letter in `takes_arg` or `optional_arg` ends the cluster
    because the rest of it is that letter's value. Returns (name, value)
    pairs; value is None when the option takes none or it is missing.
    Values of options that are not listed are not skipped, so a value that
    looks like an option can only add matches, never hide one."""
    hits = []
    for i, a in enumerate(args):
        if a == "--":
            break
        nxt = args[i + 1] if i + 1 < len(args) else None
        if a.startswith("--") and len(a) > 2:
            name, eq, value = a[2:].partition("=")
            if not name or name.startswith("no-"):
                continue
            for full, wants in longs.items():
                if full.startswith(name):
                    hits.append((full, value if eq else (nxt if wants else None)))
        elif a.startswith("-") and len(a) > 1:
            cluster = a[1:]
            for pos, ch in enumerate(cluster):
                if ch in shorts:
                    value = None
                    if ch in takes_arg:
                        value = cluster[pos + 1:] or nxt
                    hits.append(("-" + ch, value))
                if ch in takes_arg or ch in optional_arg:
                    break
    return hits


def scan_gh_flags(args, value_shorts, value_longs):
    """pflag-style scan for gh: `--name value`, `--name=value`, `-X value`,
    `-Xvalue`, `-X=value`, and boolean shorthands clustered before one value
    shorthand. No abbreviations. Returns (flags, positionals) where flags is a
    list of (name, value) with name `--long` or `-X`."""
    flags = []
    positionals = []
    i = 0
    while i < len(args):
        a = args[i]
        i += 1
        if a == "--":
            positionals.extend(args[i:])
            break
        if a.startswith("--") and len(a) > 2:
            name, eq, value = a[2:].partition("=")
            if name in value_longs and not eq:
                value = args[i] if i < len(args) else None
                i += 1
            flags.append(("--" + name, value if (eq or name in value_longs) else None))
        elif a.startswith("-") and len(a) > 1:
            cluster = a[1:]
            for pos, ch in enumerate(cluster):
                if ch in value_shorts:
                    value = cluster[pos + 1:]
                    if value.startswith("="):
                        value = value[1:]
                    if not value:
                        value = args[i] if i < len(args) else None
                        i += 1
                    flags.append(("-" + ch, value))
                    break
                flags.append(("-" + ch, None))
        else:
            positionals.append(a)
    return flags, positionals


# --------------------------------------------------------------------------
# Verdict and git context
# --------------------------------------------------------------------------

class Verdict:
    LEVELS = {"none": 0, "ask": 1, "deny": 2}

    def __init__(self):
        self.level = "none"
        self.reasons = []

    def add(self, level, reason):
        if reason not in self.reasons:
            self.reasons.append(reason)
        if self.LEVELS[level] > self.LEVELS[self.level]:
            self.level = level

    def ask(self, reason):
        self.add("ask", reason)

    def deny(self, reason):
        self.add("deny", reason)


class Repo:
    """Read-only git queries for one directory (cached per hook run)."""

    _cache = {}

    def __init__(self, cwd, extra):
        self.cwd = cwd
        self.extra = extra
        self.memo = {}

    @classmethod
    def get(cls, cwd, extra=()):
        key = (cwd, tuple(extra))
        if key not in cls._cache:
            cls._cache[key] = Repo(cwd, list(extra))
        return cls._cache[key]

    def run(self, *args):
        if not os.path.isdir(self.cwd):
            return None
        env = dict(os.environ, GIT_OPTIONAL_LOCKS="0", LC_ALL="C")
        for var in ("GIT_DIR", "GIT_WORK_TREE", "GIT_CONFIG_PARAMETERS",
                    "GIT_CONFIG_COUNT", "GIT_INDEX_FILE"):
            env.pop(var, None)
        try:
            proc = subprocess.run(
                ["git", *self.extra, *args], cwd=self.cwd, env=env,
                stdin=subprocess.DEVNULL, capture_output=True, text=True,
                timeout=4)
        except (OSError, subprocess.SubprocessError):
            return None
        return proc.stdout if proc.returncode == 0 else None

    def is_repo(self):
        if "is_repo" not in self.memo:
            self.memo["is_repo"] = self.run("rev-parse", "--git-dir") is not None
        return self.memo["is_repo"]

    def roots(self):
        """The working tree root and the main worktree root, resolved."""
        if "roots" not in self.memo:
            out = self.run("rev-parse", "--path-format=absolute",
                           "--show-toplevel", "--git-common-dir") or ""
            lines = [line for line in out.splitlines() if line]
            roots = []
            if lines:
                roots.append(os.path.realpath(lines[0]))
            if len(lines) > 1 and os.path.basename(lines[1]) == ".git":
                roots.append(os.path.realpath(os.path.dirname(lines[1])))
            self.memo["roots"] = roots
        return self.memo["roots"]

    def config(self):
        if "config" not in self.memo:
            entries = {}
            raw = self.run("config", "--list", "-z") or ""
            for item in raw.split("\0"):
                if not item:
                    continue
                key, _, value = item.partition("\n")
                entries.setdefault(key, []).append(value)
            self.memo["config"] = entries
        return self.memo["config"]

    def config_last(self, key):
        values = self.config().get(key)
        return values[-1] if values else None

    def github_repos(self):
        """owner/repo for every remote URL that points at github.com."""
        found = set()
        for key, values in self.config().items():
            if re.fullmatch(r"remote\..+\.(url|pushurl)", key):
                for url in values:
                    m = re.search(r"github\.com[:/]+([^/]+/[^/]+?)(?:\.git)?/?$", url)
                    if m:
                        found.add(m.group(1).lower())
        return found

    def current_branch(self):
        if "branch" not in self.memo:
            ref = (self.run("symbolic-ref", "-q", "HEAD") or "").strip()
            self.memo["branch"] = ref[len("refs/heads/"):] if ref.startswith("refs/heads/") else None
        return self.memo["branch"]

    def unpushed_github(self, revs, wide):
        """True when commits not on any remote touch .github/. `wide` also
        counts uncommitted and untracked files there, for a push that follows
        an add or commit in the same command."""
        shas = []
        for rev in revs:
            sha = (self.run("rev-parse", "--verify", "--quiet", "--end-of-options",
                            rev + "^{commit}") or "").strip()
            if re.fullmatch(r"[0-9a-f]{40,64}", sha):
                shas.append(sha)
        if shas:
            out = self.run("log", "--format=", "--name-only", "--max-count=500",
                           *shas, "--not", "--remotes", "--", ":(top).github")
            if out and out.strip():
                return True
        if wide:
            out = self.run("status", "--porcelain", "--untracked-files=all",
                           "--", ":(top).github")
            if out and out.strip():
                return True
        return False


# --------------------------------------------------------------------------
# Analysis
# --------------------------------------------------------------------------

def expand_vars(word, known):
    """Substitute $NAME / ${NAME} set earlier in the same command text.
    Anything unknown is left as-is and treated as unresolved later."""
    if "$" not in word:
        return word

    def sub(m):
        name = m.group(1) or m.group(2)
        return known[name] if name in known else m.group(0)
    return re.sub(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)", sub, word)


def unresolved(word):
    return "$" in word or "`" in word


def basename(word):
    return word.rsplit("/", 1)[-1]


def is_protected(name):
    return name.lower() in (PROTECTED_BRANCH, "head", "@")


def dst_candidates(dst):
    """Branch names on the remote that a destination can resolve to."""
    d = re.sub(r"/+", "/", dst)
    if d.startswith("refs/"):
        rest = d[len("refs/"):]
        return [rest[len("heads/"):]] if rest.startswith("heads/") else []
    names = [d]
    if d.startswith("heads/"):
        names.append(d[len("heads/"):])
    return names


def src_branch(src):
    """Remote branch name a `<src>` refspec (no colon) updates."""
    d = re.sub(r"/+", "/", src)
    if d.startswith("refs/"):
        rest = d[len("refs/"):]
        return rest[len("heads/"):] if rest.startswith("heads/") else None
    return d[len("heads/"):] if d.startswith("heads/") else d


def resolve_long(name):
    """git parse-options: exact match or unique prefix. Returns the option
    name, or None if unknown/ambiguous."""
    if name in PUSH_LONG_OPTS:
        return name
    hits = [opt for opt in PUSH_LONG_OPTS if opt.startswith(name)]
    return hits[0] if len(hits) == 1 else None


def git_env_risky(name):
    if name in SHARED_RISKY_ENV:
        return True
    return (name.startswith("GIT_") and name not in HARMLESS_GIT_ENV
            and not name.startswith("GIT_TRACE"))


def gh_env_risky(name):
    return name in GH_RISKY_ENV or name in ("HOME", "XDG_CONFIG_HOME")


def assignment_name(word):
    m = ASSIGNMENT.match(word)
    return m.group(0).split("[")[0].rstrip("+=") if m else None


def normalize_repo(value):
    v = re.sub(r"^[a-z]+://", "", value.strip().lower())
    v = re.sub(r"\.git/?$", "", v).strip("/")
    if v.startswith("github.com/"):
        v = v[len("github.com/"):]
    return v


def opt_label(name):
    return name if name.startswith("-") else "--" + name


class Analyzer:
    def __init__(self, verdict):
        self.verdict = verdict
        # State that earlier commands in the same text leave for later ones.
        self.config_changed = set()   # git subcommands that may change config
        self.head_changed = set()     # git subcommands that may switch branches
        self.commits_changed = False  # a commit, merge, reset, ... ran earlier
        self.global_env = set()       # risky variables assigned or exported
        self.env_stack = []           # risky prefix assignments per command

    def env_names(self):
        names = set(self.global_env)
        for frame in self.env_stack:
            names |= frame
        return names

    def opaque(self, state, cmd, reason):
        """Something runs that the guard cannot see. Ask when the text it came
        from mentions git, gh or push: the whole command the agent typed, or,
        inside a script file, the command's own line."""
        text = cmd.src if state["script"] else state["text"]
        if MENTIONS_GIT.search(text):
            self.verdict.ask(reason)

    # -- shell level ------------------------------------------------------

    def analyze_text(self, text, cwd, depth, script=False):
        if depth > MAX_DEPTH:
            raise ParseError("nesting too deep")
        state = {"cwd": cwd, "vars": {}, "text": text, "script": script}
        for cmd in parse_commands(text, depth):
            self.analyze_command(cmd, state, depth)

    def analyze_command(self, cmd, state, depth):
        known = dict(state["vars"])
        known.setdefault("PWD", state["cwd"])
        known.setdefault("HOME", os.path.expanduser("~"))
        flags = cmd.expanded
        words = [expand_vars(w, known) if e else w for w, e in zip(cmd.words, flags)]
        idx = 0
        while idx < len(words) and (ASSIGNMENT.match(words[idx]) or words[idx] in RESERVED):
            idx += 1
        assigns = [w for w in words[:idx] if ASSIGNMENT.match(w)]
        if idx < len(words) and words[idx] in ("export", "local", "declare", "readonly", "typeset"):
            for w in words[idx + 1:]:
                name = assignment_name(w) or w
                if git_env_risky(name) or gh_env_risky(name):
                    self.global_env.add(name)
            idx = len(words)
            assigns = [w for w in words if ASSIGNMENT.match(w)]
        if idx >= len(words):
            for w in assigns:
                name = assignment_name(w)
                state["vars"][name] = w[ASSIGNMENT.match(w).end():]
                if git_env_risky(name) or gh_env_risky(name):
                    self.global_env.add(name)
            return
        frame = {n for n in map(assignment_name, assigns)
                 if git_env_risky(n) or gh_env_risky(n)}
        self.env_stack.append(frame)
        try:
            self.dispatch(words, flags, idx, cmd, state, depth, frame)
        finally:
            self.env_stack.pop()

    def dispatch(self, words, flags, idx, cmd, state, depth, frame):
        head_word = words[idx]
        head = basename(head_word)
        if flags[idx] and unresolved(head_word):
            self.opaque(state, cmd, "the command name comes from a shell expansion "
                                    "the guard cannot resolve")
        if head in ("cd", "pushd"):
            args = [w for w in words[idx + 1:] if not w.startswith("-")]
            target = args[0] if args else os.path.expanduser("~")
            state["cwd"] = os.path.normpath(os.path.join(state["cwd"], os.path.expanduser(target)))
            return
        if head in NON_EXEC or head in ("for", "select"):
            return
        if head == "read":
            self.model_read(words[idx + 1:], cmd, state)
            return
        if head in ("eval", "watch"):
            rest = [w for w in words[idx + 1:] if not w.startswith("-")]
            if any(e and unresolved(w) for w, e in zip(words[idx + 1:], flags[idx + 1:])):
                self.opaque(state, cmd, f"{head} runs a command string that comes from "
                                        "a shell expansion the guard cannot resolve")
            self.analyze_text(" ".join(rest), state["cwd"], depth + 1)
            return
        if head in ("source", "."):
            if idx + 1 < len(words):
                self.analyze_script(words[idx + 1], state, depth, cmd)
            return
        for k in range(idx, len(words)):
            name = basename(words[k])
            if k > idx and head == "env" and ASSIGNMENT.match(words[k]):
                env_name = assignment_name(words[k])
                if git_env_risky(env_name) or gh_env_risky(env_name):
                    frame.add(env_name)
                continue
            if name == "git":
                wrapper = head if k > idx else None
                self.analyze_git(words[k + 1:], cmd, state["cwd"], wrapper, depth)
                return
            if name == "gh":
                self.analyze_gh(words[k + 1:], cmd, state["cwd"],
                                runs=k == idx or head in WRAPPERS)
                return
            if name in SHELLS:
                self.analyze_shell(words[k + 1:], flags[k + 1:], cmd, state, depth)
                return
            if name in INTERPRETERS:
                program = words[k + 1:] + cmd.heredocs
                if name in AWK_NAMES and self.awk_runs_commands(words[k + 1:], state["cwd"]):
                    self.verdict.ask(f"{name} program runs shell commands (system, "
                                     "getline or print to a pipe)")
                if any(INLINE_GIT_PUSH.search(w) for w in program):
                    self.verdict.ask(f"{name} program text mentions git push; "
                                     "the guard cannot resolve it")
                elif cmd.piped_in and not any(not w.startswith("-") for w in words[k + 1:]):
                    self.opaque(state, cmd, f"{name} reads its program from a pipe; "
                                            "the guard cannot see what it runs")
                return
            if k == idx and "/" in words[k]:
                self.analyze_script(words[k], state, depth, cmd, shebang=True)
                return

    def awk_runs_commands(self, args, cwd):
        """True when the awk program (inline, or from -f files) can run a
        shell command. Options with a separate value are skipped."""
        programs = []
        i = 0
        while i < len(args):
            a = args[i]
            if a in ("-f", "--file") and i + 1 < len(args):
                programs.append(self.read_file(args[i + 1], cwd) or "system(")
                i += 2
                continue
            if a in ("-v", "-F", "--assign", "--field-separator") and i + 1 < len(args):
                i += 2
                continue
            if a.startswith("-") and a != "-":
                i += 1
                continue
            if not programs:
                programs.append(a)
            break
        # String literals are data: "a | b" is not a pipe.
        return any(AWK_EXEC.search(re.sub(r'"(?:[^"\\\n]|\\.)*"', '""', p)) for p in programs)

    def model_read(self, args, cmd, state):
        """`read VAR <<< 'text'`: remember VAR so a later "$VAR" resolves."""
        names = []
        skip = False
        for a in args:
            if skip:
                skip = False
            elif a in ("-d", "-n", "-N", "-p", "-t", "-u", "-a", "-i"):
                skip = True
            elif not a.startswith("-") and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", a):
                names.append(a)
        if not names:
            return
        if not cmd.heredocs:
            for n in names:
                state["vars"].pop(n, None)
            return
        line = cmd.heredocs[0].split("\n", 1)[0]
        parts = line.split(None, len(names) - 1) if len(names) > 1 else [line.strip()]
        for n, value in zip(names, parts + [""] * len(names)):
            state["vars"][n] = value

    def analyze_shell(self, args, flags, cmd, state, depth):
        cwd = state["cwd"]
        i = 0
        while i < len(args):
            a = args[i]
            if a == "--":
                i += 1
                break
            if a in ("-o", "+o", "-O", "+O", "--rcfile", "--init-file"):
                i += 2
                continue
            if a.startswith("--"):
                i += 1
                continue
            if a[:1] in ("-", "+") and len(a) > 1:
                if "c" in a[1:]:
                    j = i + 1
                    while j < len(args) and args[j][:1] == "-" and args[j] != "-":
                        j += 1
                    if j < len(args):
                        if flags[j] and unresolved(args[j]):
                            self.opaque(state, cmd, "a shell runs a command string that "
                                                    "comes from a shell expansion the guard "
                                                    "cannot resolve")
                        self.analyze_text(args[j], cwd, depth + 1)
                    return
                i += 1
                continue
            break
        if i < len(args) and args[i] != "-":
            self.analyze_script(args[i], state, depth, cmd)
            return
        if cmd.heredocs:
            for body in cmd.heredocs:
                self.analyze_text(body, cwd, depth + 1)
        elif cmd.stdin_files:
            for path in cmd.stdin_files:
                self.analyze_script(path, state, depth, cmd)
        elif cmd.piped_in:
            self.verdict.ask("a shell reads commands from a pipe; the guard cannot "
                             "see what it runs")

    def analyze_script(self, path, state, depth, cmd, shebang=False):
        if unresolved(path):
            self.opaque(state, cmd, f"runs a script whose path ({path}) comes from a "
                                    "shell expansion the guard cannot resolve")
            return
        full = os.path.join(state["cwd"], os.path.expanduser(path))
        try:
            if not os.path.isfile(full) or os.path.getsize(full) > FILE_LIMIT:
                return
            with open(full, encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError:
            return
        if shebang:
            first = text.split("\n", 1)[0]
            if not first.startswith("#!") or not re.search(r"\b(sh|bash|zsh|dash|ksh)\b", first):
                return
        self.analyze_text(text, state["cwd"], depth + 1, script=True)

    # -- files that a command publishes ------------------------------------

    def inside_repo(self, path, cwd):
        """Inside the working tree, the main checkout, or this project's
        Claude Code temp directory (session scratchpads)."""
        full = os.path.realpath(os.path.join(cwd, os.path.expanduser(path)))
        roots = Repo.get(cwd).roots()
        for root in roots + claude_tmp_roots(roots):
            if full == root or full.startswith(root + os.sep):
                return True
        return False

    def check_input(self, path, cmd, cwd, what):
        """`what` reads `path` and sends its content to GitHub or into a
        commit that will be pushed. Files inside the repository are fine;
        anything else could carry a secret out, so ask."""
        v = self.verdict
        if path is None:
            return
        if path == "-":
            if cmd.heredocs:
                return
            for f in cmd.stdin_files:
                self.check_input(f, Command(), cwd, what)
            if cmd.piped_in:
                v.ask(f"{what} reads a pipe; the guard cannot see what it publishes")
            return
        if unresolved(path):
            v.ask(f"{what} reads a file whose path comes from a shell expansion")
        elif not self.inside_repo(path, cwd):
            v.ask(f"{what} reads {path}, which is outside the repository and the "
                  "session scratchpad (the repository is public)")

    def check_download_dir(self, path, cwd):
        """Artifacts can come from any workflow run, including fork PRs, so
        they must not land where files run as code or configure tools."""
        if unresolved(path):
            self.verdict.ask("gh run download writes to a directory from a shell expansion")
            return
        full = os.path.realpath(os.path.join(cwd, os.path.expanduser(path)))
        parts = full.split(os.sep)
        if not self.inside_repo(path, cwd) or any(p in PROTECTED_DIRS for p in parts):
            self.verdict.ask(f"gh run download writes artifacts to {path}, outside the "
                             "working tree or into a directory whose files run as code")

    def read_repo_file(self, path, cwd):
        if path == "-" or unresolved(path) or not self.inside_repo(path, cwd):
            return None
        return self.read_file(path, cwd)

    # -- git level ----------------------------------------------------------

    def analyze_git(self, args, cmd, cwd, wrapper, depth):
        i = 0
        workdir = cwd
        extra = []
        inline_config = []
        while i < len(args):
            a = args[i]
            if a == "-C" and i + 1 < len(args):
                workdir = os.path.normpath(os.path.join(workdir, os.path.expanduser(args[i + 1])))
                i += 2
                continue
            if a == "-c" and i + 1 < len(args):
                inline_config.append(args[i + 1])
                i += 2
                continue
            name, eq, value = a.partition("=")
            if name in GIT_GLOBAL_WITH_ARG:
                if not eq:
                    value = args[i + 1] if i + 1 < len(args) else ""
                    i += 1
                i += 1
                if name == "--config-env":
                    inline_config.append(value)
                elif name in ("--git-dir", "--work-tree"):
                    extra.append(f"{name}={value}")
                continue
            if a.startswith("-"):
                i += 1
                continue
            break
        if i >= len(args):
            return
        sub, rest = args[i], args[i + 1:]
        if unresolved(sub):
            self.verdict.ask(f"git subcommand '{sub}' comes from a shell expansion")
            return
        repo = Repo.get(workdir, extra)
        if sub == "push" and unresolved(workdir):
            self.verdict.ask("git push runs in a directory that comes from a shell expansion")
        if sub not in GIT_BUILTINS and sub not in ("push", "send-pack", "http-push", "subtree"):
            self.expand_git_alias(sub, rest, args[:i], inline_config, cmd, cwd,
                                  repo, wrapper, depth)
            return
        if any(a == "--output" or a.startswith("--output=") for a in rest):
            self.verdict.ask(f"git {sub} --output writes a file at any path, "
                             "including protected ones")
        if sub == "push":
            self.analyze_push(rest, repo, wrapper, inline_config)
        elif sub in ("send-pack", "http-push"):
            self.verdict.ask(f"git {sub} updates remote refs directly")
        elif sub == "subtree" and "push" in rest:
            self.verdict.ask("git subtree push updates a remote branch")
        else:
            self.check_git_options(sub, rest, cmd, workdir)
        self.record_git_effects(sub, rest)

    def expand_git_alias(self, sub, rest, prefix, inline_config, cmd, cwd,
                         repo, wrapper, depth):
        alias = None
        for item in inline_config:
            key, _, value = item.partition("=")
            if key.lower() == f"alias.{sub.lower()}":
                alias = value
        if alias is None:
            alias = repo.config_last(f"alias.{sub.lower()}")
        if not alias:
            if self.config_changed or any(git_env_risky(n) for n in self.env_names()):
                self.verdict.ask(f"git {sub} is not a git command; it may be an alias "
                                 "defined earlier in this command")
            return
        if depth >= MAX_DEPTH:
            return
        if alias.startswith("!"):
            text = alias[1:] + "".join(" " + shlex.quote(r) for r in rest)
            self.analyze_text(text, repo.cwd, depth + 1)
            return
        try:
            expanded = shlex.split(alias)
        except ValueError:
            self.verdict.ask(f"git alias '{sub}' could not be parsed")
            return
        self.analyze_git(prefix + expanded + rest, cmd, cwd, wrapper, depth + 1)

    def check_git_options(self, sub, args, cmd, cwd):
        """Dangerous options of git subcommands other than push, read with
        git's abbreviation and cluster rules."""
        v = self.verdict
        if sub == "rebase":
            if scan_git_options(args, {"exec": True}, "x", "sXxC", "S"):
                v.ask("git rebase --exec runs a shell command after each commit")
        elif sub in ("fetch", "pull"):
            if scan_git_options(args, {"upload-pack": True}):
                v.ask(f"git {sub} --upload-pack runs a command for local and ssh remotes")
            forced = scan_git_options(args, {"force": False, "update-head-ok": False},
                                      "fu", "jo" if sub == "fetch" else "sXjo", "S")
            positional = [a for a in args if not a.startswith("-")]
            if any(":" in p and p.split(":", 1)[1] and (forced or p.startswith("+"))
                   for p in positional):
                v.ask(f"git {sub} with a forced refspec can reset local branches")
        elif sub in ("checkout", "switch"):
            if sub == "checkout":
                hits = scan_git_options(args, {"force": False, "merge": False,
                                               "conflict": True}, "fmB", "bB")
            else:
                hits = scan_git_options(args, {"force": False, "discard-changes": False,
                                               "merge": False, "conflict": True,
                                               "force-create": True}, "fmC", "cC")
            if hits:
                v.ask(f"git {sub} {opt_label(hits[0][0])} can discard uncommitted "
                      "changes or reset a branch")
        elif sub == "add":
            if scan_git_options(args, {"force": False}, "f"):
                v.ask("git add --force stages files that .gitignore excludes, such "
                      "as secrets")
        elif sub == "commit":
            for name, value in scan_git_options(
                    args, {"file": True, "template": True}, "Ft", "mFcCt", "uS"):
                what = "git commit --template" if name in ("template", "-t") else "git commit --file"
                self.check_input(value, cmd, cwd, what)
        elif sub == "branch":
            hits = scan_git_options(args, {"force": False}, "DfMC", "u")
            if hits:
                v.ask("git branch -D/-f/-M/-C can drop commits or overwrite a branch")
        elif sub == "worktree" and args:
            action, opts = args[0], args[1:]
            if action == "remove" and scan_git_options(opts, {"force": False}, "f"):
                v.ask("git worktree remove --force deletes uncommitted changes in "
                      "that worktree")
            if action == "add" and scan_git_options(opts, {}, "B", "bB"):
                v.ask("git worktree add -B resets an existing branch")

    def record_git_effects(self, sub, args):
        """Remember what this git command changes, for later pushes in the
        same text: the guard reads the repository as it is before the whole
        command runs."""
        positional = [a for a in args if not a.startswith("-")]
        if sub == "config":
            reads = {"--get", "--get-all", "--get-regexp", "--get-urlmatch", "--list",
                     "-l", "--get-color", "--get-colorbool"}
            if positional[:1] in (["get"], ["list"]) or any(a in reads for a in args):
                return
            if len(positional) <= 1 and not any(a.startswith("--") and a not in
                                                ("--global", "--local", "--system",
                                                 "--worktree", "--show-origin",
                                                 "--show-scope", "--null", "-z")
                                                for a in args):
                return
            self.config_changed.add("config")
        elif sub == "remote":
            if not positional or positional[0] in ("show", "get-url"):
                return
            self.config_changed.add("remote")
        elif sub == "branch":
            changes = scan_git_options(args, {
                "delete": False, "move": False, "copy": False, "force": False,
                "set-upstream-to": True, "unset-upstream": False, "track": False,
                "edit-description": False}, "dDmMcCfut", "u")
            filters = ("--contains", "--no-contains", "--merged", "--no-merged",
                       "--points-at", "--format", "--sort", "--list", "-l", "--column")
            creates = positional and not any(a.startswith(filters) for a in args)
            if not changes and not creates:
                return  # a listing
            # Can rename the current branch or set its upstream.
            self.head_changed.add("branch")
            self.commits_changed = True
        elif sub == "worktree":
            if positional[:1] in (["list"], ["prune"], []):
                return
            # A new branch with an upstream; matters for push.default=upstream.
            self.head_changed.add("worktree " + positional[0])
        elif sub in GIT_HEAD_CHANGERS:
            self.head_changed.add(sub)
            self.commits_changed = True
        elif sub not in GIT_READ_ONLY and not (sub == "stash" and positional[:1] in (["list"], ["show"])):
            self.commits_changed = True

    def analyze_push(self, args, repo, wrapper, inline_config):
        v = self.verdict
        if wrapper in ARG_WRAPPERS:
            v.ask(f"git push arguments are supplied by {wrapper}; the guard "
                  "cannot see the destinations")
        if inline_config:
            v.ask("inline -c/--config-env on git push can change where it pushes")
        env = sorted(n for n in self.env_names() if git_env_risky(n))
        if env:
            v.ask(f"{', '.join(env)} set in this command can change where git pushes")
        if self.config_changed:
            v.ask(f"an earlier command in this text changes git config or branches "
                  f"({', '.join(sorted(self.config_changed))}); the guard resolves "
                  "pushes against the repository as it was before the command")

        flags = {"force": False, "lease": False, "delete": False,
                 "dry": False, "tags": False, "all": False, "prune": False,
                 "receive": False}
        positionals = []
        i = 0
        while i < len(args):
            a = args[i]
            i += 1
            if a == "--":
                positionals.extend(args[i:])
                break
            if a.startswith("--"):
                name, eq, _ = a[2:].partition("=")
                negated = name.startswith("no-")
                opt = resolve_long(name[3:] if negated else name)
                if opt is None:
                    if not negated:
                        v.ask(f"unrecognised or ambiguous git push option '{a}'")
                    continue
                if opt in PUSH_OPTS_WITH_ARG and not eq and not negated:
                    i += 1
                if opt == "force":
                    flags["force"] = not negated
                elif opt == "force-with-lease":
                    flags["lease"] = not negated
                elif opt == "delete":
                    flags["delete"] = not negated
                elif opt == "dry-run":
                    flags["dry"] = not negated
                elif opt == "tags":
                    flags["tags"] = not negated
                elif opt in ("all", "branches", "mirror"):
                    flags["all"] = flags["all"] or not negated
                elif opt == "prune":
                    flags["prune"] = not negated
                elif opt in ("receive-pack", "exec"):
                    flags["receive"] = True
                continue
            if a.startswith("-") and len(a) > 1:
                cluster = a[1:]
                for pos, ch in enumerate(cluster):
                    if ch == "f":
                        flags["force"] = True
                    elif ch == "d":
                        flags["delete"] = True
                    elif ch == "n":
                        flags["dry"] = True
                    elif ch == "o":
                        if pos == len(cluster) - 1:
                            i += 1
                        break
                continue
            positionals.append(a)

        if flags["dry"]:
            return
        if any(unresolved(p) for p in positionals):
            v.ask("a push remote or refspec comes from a shell expansion the guard "
                  "cannot resolve")
        if flags["receive"]:
            v.ask("--receive-pack/--exec runs a command for local remotes")
        if not inline_config and not env and remote_is_local(repo, positionals[0] if positionals else None):
            return
        if flags["all"]:
            v.deny("--all/--branches/--mirror pushes every branch, including main")
        if flags["prune"]:
            v.ask("--prune deletes remote branches that have no local counterpart")
        if flags["force"]:
            v.deny("plain force push (--force/-f); use --force-with-lease on a claude/* branch")

        config = repo.config()
        if any(re.fullmatch(r"remote\..+\.push", k) for k in config):
            v.ask("remote.<name>.push is configured and can remap push destinations")
        if any(re.fullmatch(r"remote\..+\.mirror", k) and vals[-1].lower() in ("true", "yes", "on", "1")
               for k, vals in config.items()):
            v.ask("a remote is configured with mirror=true")

        refspecs = positionals[1:]
        push_default = (repo.config_last("push.default") or "simple").strip()
        follow_upstream = push_default in ("upstream", "tracking")
        head_dependent = not positionals
        dests = []
        deletes = []
        srcs = []

        def current():
            if not repo.is_repo():
                v.ask("cannot resolve the current branch for this push "
                      "(directory is not a git repository yet)")
                return None
            return repo.current_branch()

        def upstream_of(branch):
            merge = repo.config_last(f"branch.{branch}.merge") or ""
            return merge[len("refs/heads/"):] if merge.startswith("refs/heads/") else None

        if flags["delete"]:
            deletes.extend(r.lstrip("+") for r in refspecs)
        elif not refspecs and not flags["tags"]:
            head_dependent = True
            if push_default == "matching":
                v.deny("implicit push with push.default=matching pushes every "
                       "matching branch, including main")
            elif push_default != "nothing":
                cur = current()
                if cur:
                    dests.append((cur, [cur]))
                    srcs.append("HEAD")
                    if follow_upstream and upstream_of(cur):
                        up = upstream_of(cur)
                        dests.append((up, [up]))
        else:
            it = iter(refspecs)
            for spec in it:
                if spec == "tag":
                    next(it, None)
                    continue
                if spec.startswith("+"):
                    v.deny(f"'+' refspec force-pushes ({spec})")
                    spec = spec[1:]
                if spec in (":", ""):
                    v.deny("the ':' refspec pushes every matching branch, including main")
                    continue
                if ":" in spec:
                    src, dst = spec.split(":", 1)
                    if not src:
                        deletes.append(dst)
                        continue
                    dests.append((dst, dst_candidates(dst or src)))
                    srcs.append(src)
                    continue
                srcs.append(spec)
                if spec in ("HEAD", "@"):
                    head_dependent = True
                    cur = current()
                    if cur:
                        dests.append((cur, [cur]))
                    elif repo.is_repo():
                        v.ask("HEAD is detached; cannot resolve the push destination")
                    continue
                branch = src_branch(spec)
                if branch is None:
                    continue
                dests.append((spec, [branch]))
                if follow_upstream and upstream_of(branch):
                    up = upstream_of(branch)
                    dests.append((up, [up]))

        if self.head_changed and (head_dependent or follow_upstream):
            v.ask(f"an earlier command in this text can switch branches "
                  f"({', '.join(sorted(self.head_changed))}); the guard resolves "
                  "HEAD and upstreams as they were before the command")
        for shown, names in dests:
            if any(glob_hits_protected(n) for n in names):
                v.deny(f"push would update {PROTECTED_BRANCH} (destination '{shown}')")
        for shown in deletes:
            names = dst_candidates(shown)
            if any(glob_hits_protected(n) for n in names):
                v.deny(f"push would delete {PROTECTED_BRANCH} ('{shown}')")
            else:
                v.ask(f"push deletes remote ref '{shown}'")
        if flags["lease"]:
            for shown, names in dests:
                if not names or not all(n.startswith(AGENT_PREFIX) for n in names):
                    v.ask(f"--force-with-lease rewrites '{shown}', which is outside {AGENT_PREFIX}*")
        if v.level != "deny" and srcs and repo.is_repo():
            if repo.unpushed_github(srcs, self.commits_changed):
                v.ask("commits for this push change .github/; CI runs those workflows "
                      "with repository secrets")

    # -- gh level -------------------------------------------------------------

    def analyze_gh(self, args, cmd, cwd, runs=True):
        """`runs` is False when `gh` is only an argument of some other command
        (`ln -s x/gh y`); then only recognised gh commands are checked."""
        v = self.verdict
        if not args:
            return
        top = args[0]
        if top not in GH_COMMANDS:
            if runs and unresolved(top):
                v.ask(f"gh command '{top}' comes from a shell expansion")
            elif runs and not top.startswith("-"):
                v.ask(f"gh {top} is an alias or extension; the guard cannot see what it runs")
            return
        sub = args[1] if len(args) > 1 else ""
        env = sorted(n for n in self.env_names() if gh_env_risky(n))
        if top == "alias" and sub in ("set", "import", "delete"):
            v.ask("gh alias changes can hide a merge or an API write behind a new name")
        elif top == "pr" and sub == "merge":
            if any(a == "--admin" or a.startswith("--admin=") for a in args[2:]):
                v.deny("gh pr merge --admin bypasses branch protection; merging is "
                       "reserved for the user")
            else:
                v.ask("gh pr merge: merging is reserved for the user")
        elif (top, sub) in GH_PUBLISH:
            self.check_gh_publish(top, sub, args[2:], cmd, cwd, env)
        elif top == "run" and sub == "download":
            flags, _ = scan_gh_flags(args[2:], "Dnp", {"dir", "name", "pattern", "repo"})
            for name, value in flags:
                if name in ("-D", "--dir") and value is not None:
                    self.check_download_dir(value, cwd)
        elif top == "api":
            self.check_gh_api(args[1:], cmd, cwd, env)

    def check_home_repo(self, what, repo_flag, cwd, env):
        v = self.verdict
        if env:
            v.ask(f"{', '.join(env)} set for {what} can point it at another repository")
        if repo_flag is not None:
            if unresolved(repo_flag):
                v.ask(f"{what} targets a repository from a shell expansion")
            elif normalize_repo(repo_flag) != HOME_REPO.lower():
                v.ask(f"{what} targets {repo_flag}, not {HOME_REPO}")
            return
        repo = Repo.get(cwd)
        if repo.is_repo():
            found = repo.github_repos()
            if found and HOME_REPO.lower() not in found:
                v.ask(f"{what} runs in a checkout of {', '.join(sorted(found))}, not {HOME_REPO}")

    def check_gh_publish(self, top, sub, args, cmd, cwd, env):
        v = self.verdict
        what = f"gh {top} {sub}"
        flags, _ = scan_gh_flags(args, GH_PUBLISH[(top, sub)],
                                 {"repo", "body-file", "body", "title", "base", "head",
                                  "assignee", "label", "milestone", "project",
                                  "reviewer", "template"})
        repo_flag = None
        for name, value in flags:
            if name in ("-R", "--repo"):
                repo_flag = value or ""
            elif name in ("-F", "--body-file"):
                self.check_input(value, cmd, cwd, what)
                content = self.read_repo_file(value, cwd) if value else None
                if content and "@claude" in content.lower():
                    v.ask(f"{what} body file mentions @claude, which starts the Claude workflow")
        self.check_home_repo(what, repo_flag, cwd, env)
        if any("@claude" in a.lower() for a in args):
            v.ask(f"{what} mentions @claude, which starts the Claude workflow")

    def check_gh_api(self, args, cmd, cwd, env):
        v = self.verdict
        flags, positionals = scan_gh_flags(args, set(API_VALUE_SHORTS), API_VALUE_LONGS)
        method = None
        fields = []
        inputs = []
        headers = []
        for name, value in flags:
            key = name[2:] if name.startswith("--") else API_VALUE_SHORTS.get(name[1:])
            if value is None:
                continue
            if key == "method":
                method = value
            elif key in ("field", "raw-field"):
                fields.append((key, value))
            elif key == "input":
                inputs.append(value)
            elif key == "header":
                headers.append(value)
        endpoint = positionals[0] if positionals else ""
        if method is not None and unresolved(method):
            v.ask("gh api method comes from a shell expansion")
            return
        method = (method or ("POST" if fields or inputs else "GET")).upper()
        if any(re.match(r"\s*x-http-method-override\s*:", h, re.I) for h in headers):
            v.ask("gh api overrides the HTTP method with a header")
        for key, value in fields:
            name, _, val = value.partition("=")
            if key == "field" and val.startswith("@"):
                self.check_input(val[1:], cmd, cwd, f"gh api field {name}")
        ep = re.sub(r"^https?://api\.github\.com/", "", endpoint).lstrip("/").split("?", 1)[0]
        if env:
            v.ask(f"{', '.join(env)} set for gh api can point it at another host or repository")
        if ep == "graphql":
            self.check_graphql(fields, inputs, cmd, cwd)
            return
        for path in inputs:
            self.check_input(path, cmd, cwd, "gh api --input")
        if method in ("GET", "HEAD"):
            return
        if method in ("PUT", "PATCH", "DELETE"):
            v.ask(f"gh api {method} {endpoint or '(no endpoint)'} changes GitHub state")
            return
        if unresolved(ep) or not routine_post(ep):
            v.ask(f"gh api {method} {endpoint or '(no endpoint)'} is not a routine "
                  "comment, reply, review or reaction on this repository")
        elif "{owner}" in ep or ":owner" in ep:
            self.check_home_repo(f"gh api {method}", None, cwd, [])

    def check_graphql(self, fields, inputs, cmd, cwd):
        v = self.verdict
        texts = []
        for key, value in fields:
            name, _, val = value.partition("=")
            if key == "field" and val.startswith("@"):
                path = val[1:]
                content = cmd.heredocs[0] if path == "-" and cmd.heredocs else self.read_file(path, cwd)
                if content is None:
                    v.ask(f"gh api graphql reads {name} from {path}, which the guard cannot read")
                else:
                    texts.append(content)
            else:
                texts.append(val)
        for path in inputs:
            content = cmd.heredocs[0] if path == "-" and cmd.heredocs else self.read_file(path, cwd)
            if content is None:
                v.ask(f"gh api graphql reads its request from {path}, which the guard cannot read")
            else:
                texts.append(content)
        for text in texts:
            if unresolved(text) and "mutation" in text:
                v.ask("gh api graphql mutation text comes from a shell expansion")
            m = GQL_DANGEROUS.search(text)
            if m:
                v.ask(f"gh api graphql calls {m.group(0)}, which changes branches, "
                      "merges or repository settings")

    def read_file(self, path, cwd):
        if path == "-" or unresolved(path):
            return None
        full = os.path.join(cwd, os.path.expanduser(path))
        try:
            if os.path.getsize(full) > FILE_LIMIT:
                return None
            with open(full, encoding="utf-8", errors="replace") as fh:
                return fh.read()
        except OSError:
            return None


def claude_tmp_roots(repo_roots):
    """Claude Code keeps session scratchpads under /tmp/claude-<uid>/<slug>,
    where slug is the project path with every other character replaced by
    '-'. Files there are the agent's own drafts, like files in the repo."""
    project = os.environ.get("CLAUDE_PROJECT_DIR") or (repo_roots[-1] if repo_roots else None)
    if not project or not hasattr(os, "getuid"):
        return []
    slug = re.sub(r"[^A-Za-z0-9]", "-", project)
    return [os.path.realpath(os.path.join("/tmp", f"claude-{os.getuid()}", slug))]


def routine_post(endpoint):
    """POST endpoints the review workflow uses on this repository."""
    repo = r"(?:%s|\{owner\}/\{repo\}|:owner/:repo)" % re.escape(HOME_REPO)
    patterns = [
        rf"repos/{repo}/(?:issues|pulls)/\d+/comments",
        rf"repos/{repo}/pulls/\d+/comments/\d+/replies",
        rf"repos/{repo}/pulls/\d+/reviews",
        rf"repos/{repo}/pulls/\d+/requested_reviewers",
        rf"repos/{repo}/issues/\d+/labels",
        rf"repos/{repo}/issues/(?:comments/)?\d+/reactions",
        rf"repos/{repo}/pulls/comments/\d+/reactions",
        rf"repos/{repo}/(?:issues|pulls)",
        r"markdown(?:/raw)?",
    ]
    return any(re.fullmatch(p, endpoint.rstrip("/"), re.I) for p in patterns)


def remote_is_local(repo, remote):
    """True when every URL the push goes to is a local path, so it cannot
    reach GitHub. Unknown state (not a repo yet, url rewriting) is remote."""
    if not repo.is_repo():
        return False
    config = repo.config()
    if any(re.fullmatch(r"url\..+\.(insteadof|pushinsteadof)", k) for k in config):
        return False
    if remote is None:
        cur = repo.current_branch()
        remote = ((cur and repo.config_last(f"branch.{cur}.pushremote"))
                  or repo.config_last("remote.pushdefault")
                  or (cur and repo.config_last(f"branch.{cur}.remote"))
                  or "origin")
    urls = config.get(f"remote.{remote}.pushurl") or config.get(f"remote.{remote}.url") or [remote]
    for url in urls:
        if url.startswith("file://"):
            continue
        if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*::?", url) or re.match(r"^[^/]*:", url):
            return False
    return True


def glob_hits_protected(name):
    if "*" in name:
        import fnmatch
        return fnmatch.fnmatchcase(PROTECTED_BRANCH, name) or fnmatch.fnmatchcase(PROTECTED_BRANCH.upper(), name.upper())
    return is_protected(name)


# --------------------------------------------------------------------------
# Hook entry point
# --------------------------------------------------------------------------

def decide(command, cwd):
    verdict = Verdict()
    try:
        Analyzer(verdict).analyze_text(command, cwd, 0)
    except (ParseError, RecursionError) as exc:
        if MENTIONS_GIT.search(command):
            verdict.ask(f"git-push-guard could not parse the command ({exc})")
    return verdict


def emit(level, reason):
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": level,
        "permissionDecisionReason": reason,
    }}))
    if level == "deny":
        print(reason, file=sys.stderr)
        return 2
    return 0


def main():
    try:
        payload = json.loads(sys.stdin.read())
        if not isinstance(payload, dict):
            raise ValueError("payload is not a JSON object")
    except (ValueError, OSError, UnicodeDecodeError) as exc:
        return emit("ask", f"git-push-guard could not read the hook payload ({exc})")
    if payload.get("tool_name") not in ("Bash", "Monitor"):
        return 0
    tool_input = payload.get("tool_input") or {}
    command = tool_input.get("command") if isinstance(tool_input, dict) else None
    if not isinstance(command, str) or not command:
        return 0
    cwd = payload.get("cwd") or os.getcwd()
    try:
        verdict = decide(command, cwd)
    except Exception as exc:  # fail closed
        return emit("ask", f"git-push-guard error: {type(exc).__name__}: {exc}")
    if verdict.level == "none":
        return 0
    reason = "git-push-guard: " + "; ".join(verdict.reasons)
    if verdict.level == "deny":
        reason += (". Pushes to main, force pushes, mirror pushes and admin merges "
                   "are reserved for the user: push a claude/* branch and open a PR "
                   "instead.")
    return emit(verdict.level, reason)


if __name__ == "__main__":
    sys.exit(main())
