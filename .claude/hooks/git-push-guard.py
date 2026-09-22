#!/usr/bin/env python3
"""PreToolUse guard for the Bash tool: resolves every `git push` a command
would run and blocks the ones that must go through the user.

Claude Code's Bash permission rules match command text, so `git push origin
claude/x HEAD:heads/main`, a bare `git push` while on main, `git -c k=v push`
or `env git push` slip past them. This hook parses the command the way a
shell would (quotes, `$(...)`, backticks, heredocs, `sh -c`, `eval`, git
aliases), then resolves each push destination the way git does (bare name,
heads/X, refs/heads/X, HEAD, implicit push via push.default and the branch
upstream).

Decisions (the strictest one wins):
  deny  any push that updates or deletes main; plain force pushes (--force,
        -f, +refspec); --all, --branches, --mirror; matching pushes (":" or
        push.default=matching)
  ask   deleting any other remote branch; --prune; --force-with-lease to a
        branch outside claude/*; --receive-pack/--exec; inline -c or
        --config-env on a push; remote.<name>.push or .mirror in config;
        push arguments supplied by xargs/find/parallel; a destination that
        cannot be resolved; unpushed commits that touch .github/ (CI runs
        them with repository secrets); a shell reading commands from stdin
  none  everything else; the normal permission rules and auto mode decide.

This is defence in depth, not a security boundary: a script file, a build
step or a shell alias can still run git. The server-side ruleset on main is
the boundary. The hook fails open (no decision) on internal errors unless
the command text mentions "push", in which case it asks.
"""

import json
import os
import re
import shlex
import subprocess
import sys

PROTECTED_BRANCH = "main"
AGENT_PREFIX = "claude/"
MAX_DEPTH = 8
SCRIPT_LIMIT = 256 * 1024

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
ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(\[[^]]*\])?\+?=")
INLINE_GIT_PUSH = re.compile(r"\bgit\b.*\bpush\b", re.S)

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


class ParseError(Exception):
    pass


# --------------------------------------------------------------------------
# Shell parsing
# --------------------------------------------------------------------------

class Command:
    __slots__ = ("words", "heredocs")

    def __init__(self):
        self.words = []
        self.heredocs = []


class ShellParser:
    """Splits shell text into simple commands (lists of words with quoting
    removed). Command substitutions, process substitutions and backticks are
    parsed recursively; their commands are emitted before the command that
    contains them. Heredoc and here-string bodies are attached to their
    command as data."""

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
        self.skip_next = False
        self.herestring_next = False
        self.pending = []
        self.brace = False

    def peek(self, k=0):
        j = self.i + k
        return self.s[j] if j < self.n else ""

    def add(self, text):
        self.word = (self.word or "") + text

    def end_word(self):
        if self.word is None:
            return
        word, self.word = self.word, None
        brace, self.brace = self.brace, False
        if self.skip_next:
            self.skip_next = False
        elif self.herestring_next:
            self.herestring_next = False
            self.cmd.heredocs.append(word)
        else:
            self.cmd.words.extend(brace_expand(word) if brace else [word])

    def end_command(self):
        self.end_word()
        waiting = any(p[2] is self.cmd for p in self.pending)
        if self.cmd.words or self.cmd.heredocs or waiting:
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
                i = self.i
            elif c == "`":
                self.i = i
                buf.append(self.read_backtick())
                i = self.i
            else:
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
        """At `<`, `>` or `&>`: consume the operator; the next word is its
        target (skipped), unless it is a process substitution."""
        if self.word is not None and self.word.isdigit():
            self.word = None
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
        self.skip_next = True

    def parse(self):
        s = self.s
        while self.i < self.n:
            c = s[self.i]
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
                    self.add(self.peek(1))
                    self.i += 2
            elif c == "'":
                self.add(self.read_single())
            elif c == '"':
                self.add(self.read_double())
            elif c == "$" and self.peek(1) == "'":
                self.add(self.read_ansi_c())
            elif c == "$" and self.peek(1) == '"':
                self.i += 1  # $"..." is a translatable double-quoted string
            elif c == "$" and self.peek(1) == "(":
                self.add(self.read_substitution(2))
            elif c == "`":
                self.add(self.read_backtick())
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
            elif c in ";&|":
                self.end_command()
                self.i += 2 if s[self.i:self.i + 2] in ("&&", "||", ";;", "|&", ";&") else 1
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
    def get(cls, cwd, extra):
        key = (cwd, tuple(extra))
        if key not in cls._cache:
            cls._cache[key] = Repo(cwd, extra)
        return cls._cache[key]

    def run(self, *args):
        if not os.path.isdir(self.cwd):
            return None
        env = dict(os.environ, GIT_OPTIONAL_LOCKS="0", LC_ALL="C")
        for var in ("GIT_DIR", "GIT_WORK_TREE", "GIT_CONFIG_PARAMETERS"):
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

    def current_branch(self):
        if "branch" not in self.memo:
            ref = (self.run("symbolic-ref", "-q", "HEAD") or "").strip()
            self.memo["branch"] = ref[len("refs/heads/"):] if ref.startswith("refs/heads/") else None
        return self.memo["branch"]

    def unpushed_files(self, rev):
        sha = (self.run("rev-parse", "--verify", "--quiet", "--end-of-options",
                        rev + "^{commit}") or "").strip()
        if not re.fullmatch(r"[0-9a-f]{40,64}", sha):
            return []
        out = self.run("log", "--format=", "--name-only", "--max-count=500",
                       sha, "--not", "--remotes") or ""
        return [line for line in out.splitlines() if line]


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


class Analyzer:
    def __init__(self, verdict, raw):
        self.verdict = verdict
        self.raw = raw

    # -- shell level ------------------------------------------------------

    def analyze_text(self, text, cwd, depth):
        if depth > MAX_DEPTH:
            raise ParseError("nesting too deep")
        state = {"cwd": cwd, "vars": {}}
        for cmd in parse_commands(text, depth):
            self.analyze_command(cmd, state, depth)

    def analyze_command(self, cmd, state, depth):
        known = state["vars"]
        words = [expand_vars(w, known) for w in cmd.words]
        idx = 0
        while idx < len(words) and (ASSIGNMENT.match(words[idx]) or words[idx] in RESERVED):
            idx += 1
        assigns = words[:idx]
        if idx < len(words) and words[idx] in ("export", "local", "declare", "readonly", "typeset"):
            assigns = words[idx + 1:]
            idx = len(words)
        if idx >= len(words):
            for w in assigns:
                m = ASSIGNMENT.match(w)
                if m:
                    name = m.group(0).split("[")[0].rstrip("+=")
                    known[name] = w[m.end():]
            return
        head = basename(words[idx])
        if head in ("cd", "pushd"):
            args = [w for w in words[idx + 1:] if not w.startswith("-")]
            target = args[0] if args else os.path.expanduser("~")
            state["cwd"] = os.path.normpath(os.path.join(state["cwd"], os.path.expanduser(target)))
            return
        if head in NON_EXEC:
            return
        if head == "eval" or head == "watch":
            rest = [w for w in words[idx + 1:] if not w.startswith("-")]
            self.analyze_text(" ".join(rest), state["cwd"], depth + 1)
            return
        if head in ("source", "."):
            if idx + 1 < len(words):
                self.analyze_script(words[idx + 1], state["cwd"], depth)
            return
        for k in range(idx, len(words)):
            name = basename(words[k])
            if name == "git":
                wrapper = head if k > idx else None
                self.analyze_git(words[k + 1:], state["cwd"], wrapper, depth)
                return
            if name in SHELLS:
                self.analyze_shell(words[k + 1:], cmd, state["cwd"], depth)
                return
            if name in INTERPRETERS:
                if any(INLINE_GIT_PUSH.search(w) for w in words[k + 1:] + cmd.heredocs):
                    self.verdict.ask(f"{name} program text mentions git push; "
                                     "the guard cannot resolve it")
                return
            if k == idx and "/" in words[k]:
                self.analyze_script(words[k], state["cwd"], depth, shebang=True)
                return

    def analyze_shell(self, args, cmd, cwd, depth):
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
                        self.analyze_text(args[j], cwd, depth + 1)
                    return
                i += 1
                continue
            break
        if i < len(args) and args[i] != "-":
            self.analyze_script(args[i], cwd, depth)
            return
        if cmd.heredocs:
            for body in cmd.heredocs:
                self.analyze_text(body, cwd, depth + 1)
        elif "push" in self.raw:
            self.verdict.ask("a shell reads commands from stdin; the guard "
                             "cannot see what it runs")

    def analyze_script(self, path, cwd, depth, shebang=False):
        full = os.path.join(cwd, os.path.expanduser(path))
        try:
            if not os.path.isfile(full) or os.path.getsize(full) > SCRIPT_LIMIT:
                return
            with open(full, encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        except OSError:
            return
        if shebang:
            first = text.split("\n", 1)[0]
            if not first.startswith("#!") or not re.search(r"\b(sh|bash|zsh|dash|ksh)\b", first):
                return
        self.analyze_text(text, cwd, depth + 1)

    # -- git level ----------------------------------------------------------

    def analyze_git(self, args, cwd, wrapper, depth):
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
        if sub == "push":
            self.analyze_push(rest, repo, wrapper, inline_config)
        elif sub in ("send-pack", "http-push"):
            self.verdict.ask(f"git {sub} updates remote refs directly")
        elif sub == "subtree" and "push" in rest:
            self.verdict.ask("git subtree push updates a remote branch")
        elif sub not in GIT_BUILTINS:
            alias = None
            for item in inline_config:
                key, _, value = item.partition("=")
                if key.lower() == f"alias.{sub.lower()}":
                    alias = value
            if alias is None:
                alias = repo.config_last(f"alias.{sub.lower()}")
            if not alias or depth >= MAX_DEPTH:
                return
            prefix = args[:i]
            if alias.startswith("!"):
                text = alias[1:] + "".join(" " + shlex.quote(r) for r in rest)
                self.analyze_text(text, workdir, depth + 1)
            else:
                try:
                    expanded = shlex.split(alias)
                except ValueError:
                    self.verdict.ask(f"git alias '{sub}' could not be parsed")
                    return
                self.analyze_git(prefix + expanded + rest, cwd, wrapper, depth + 1)

    def analyze_push(self, args, repo, wrapper, inline_config):
        v = self.verdict
        if wrapper in ARG_WRAPPERS:
            v.ask(f"git push arguments are supplied by {wrapper}; the guard "
                  "cannot see the destinations")
        if inline_config:
            v.ask("inline -c/--config-env on git push can change where it pushes")

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
        if not inline_config and remote_is_local(repo, positionals[0] if positionals else None):
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
            for src in srcs:
                if any(f.startswith(".github/") for f in repo.unpushed_files(src)):
                    v.ask("unpushed commits change .github/; CI runs those workflows "
                          "with repository secrets")
                    break


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
        Analyzer(verdict, command).analyze_text(command, cwd, 0)
    except (ParseError, RecursionError) as exc:
        if "push" in command:
            verdict.ask(f"git-push-guard could not parse the command ({exc})")
    return verdict


def main():
    try:
        payload = json.load(sys.stdin)
    except (ValueError, OSError):
        return 0
    if not isinstance(payload, dict) or payload.get("tool_name") != "Bash":
        return 0
    tool_input = payload.get("tool_input") or {}
    command = tool_input.get("command") if isinstance(tool_input, dict) else None
    if not isinstance(command, str) or not command:
        return 0
    cwd = payload.get("cwd") or os.getcwd()
    try:
        verdict = decide(command, cwd)
    except Exception as exc:  # fail open, but never silently on a push
        verdict = Verdict()
        if "push" in command:
            verdict.ask(f"git-push-guard error: {type(exc).__name__}: {exc}")
    if verdict.level == "none":
        return 0
    reason = "git-push-guard: " + "; ".join(verdict.reasons)
    if verdict.level == "deny":
        reason += (". Pushes to main, force pushes and mirror pushes are reserved "
                   "for the user: push a claude/* branch and open a PR instead.")
    print(json.dumps({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": verdict.level,
        "permissionDecisionReason": reason,
    }}))
    if verdict.level == "deny":
        print(reason, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
