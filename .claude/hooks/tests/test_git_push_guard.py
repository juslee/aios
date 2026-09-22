"""Tests for .claude/hooks/git-push-guard.py.

Run: python3 -m unittest discover -s .claude/hooks/tests -v
Each test class builds throwaway git repositories in a temp directory; no
network access and nothing outside the temp directory is touched.
"""

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
HOOK = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "git-push-guard.py")
spec = importlib.util.spec_from_file_location("git_push_guard", HOOK)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def git(cwd, *args):
    env = dict(os.environ, GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1",
               GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.invalid",
               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.invalid")
    return subprocess.run(["git", *args], cwd=cwd, env=env, check=True,
                          capture_output=True, text=True).stdout


class RepoCase(unittest.TestCase):
    """A clone with main and claude/x pushed to a bare origin."""

    def setUp(self):
        os.environ["GIT_CONFIG_GLOBAL"] = os.devnull
        os.environ["GIT_CONFIG_NOSYSTEM"] = "1"
        self.tmp = tempfile.TemporaryDirectory()
        root = self.tmp.name
        self.origin = os.path.join(root, "origin.git")
        self.repo = os.path.join(root, "work")
        git(root, "init", "-q", "--bare", "-b", "main", self.origin)
        git(root, "init", "-q", "-b", "main", self.repo)
        git(self.repo, "remote", "add", "origin", self.origin)
        git(self.repo, "commit", "-q", "--allow-empty", "-m", "base")
        git(self.repo, "push", "-q", "-u", "origin", "main")
        git(self.repo, "switch", "-q", "-c", "claude/x", "--track", "origin/main")
        git(self.repo, "commit", "-q", "--allow-empty", "-m", "work")
        # The guard only reads config, so origin can point at GitHub without
        # network access; real pushes in GitSemantics go to "scratch".
        git(self.repo, "remote", "set-url", "origin", "https://github.com/example/aios.git")
        git(self.repo, "remote", "add", "scratch", self.origin)
        guard.Repo._cache.clear()

    def tearDown(self):
        self.tmp.cleanup()

    def level(self, command, cwd=None):
        guard.Repo._cache.clear()
        return guard.decide(command, cwd or self.repo).level

    def assertLevel(self, expected, command, cwd=None):
        self.assertEqual(expected, self.level(command, cwd), command)


class PushToMain(RepoCase):
    def test_explicit_main_forms_are_denied(self):
        for cmd in [
            "git push origin main",
            "git push origin HEAD:main",
            "git push origin claude/x:main",
            "git push origin claude/x main",
            "git push origin claude/x HEAD:heads/main",
            "git push origin claude/x HEAD:refs/heads/main",
            "git push origin claude/x HEAD:refs//heads//main",
            "git push -u origin claude/x HEAD:heads/main",
            "git push origin claude/x 'main'",
            "git push origin claude/x ma'i'n",
            "git push origin claude/x $'\\x6dain'",
            "git push origin claude/x HEAD:Main",
            "git push origin HEAD:HEAD",
            "git push origin 'refs/heads/*'",
            "git push origin 'refs/heads/*:refs/heads/*'",
            "git push origin :",
            "git push origin --delete main",
            "git push origin :main",
            "git push --all origin",
            "git push --branches origin",
            "git push --mirror origin",
            "git push --mir origin",
        ]:
            self.assertLevel("deny", cmd)

    def test_wrapped_and_nested_forms_are_denied(self):
        for cmd in [
            "git -C . push origin main",
            "git -C " + self.repo + " push origin claude/x HEAD:heads/main",
            "git -c core.x=y push origin main",
            "git --no-pager push origin main",
            "git --git-dir=.git push origin main",
            "env git push origin main",
            "env -i PATH=/usr/bin git push origin main",
            "FOO=1 git push origin main",
            "command git push origin main",
            "\\git push origin main",
            "/usr/bin/git push origin main",
            "timeout 30 git push origin main",
            "nohup git push origin main &",
            "sudo -u me git push origin main",
            "cd . && git push origin main",
            "true; git push origin main",
            "echo x | git push origin main",
            "(git push origin main)",
            "{ git push origin main; }",
            "for r in main; do git push origin main; done",
            "sh -c 'git push origin main'",
            "bash -lc \"cd . && git push origin HEAD:main\"",
            "zsh -c 'git push origin main'",
            "eval 'git push origin main'",
            "eval git push origin main",
            "echo $(git push origin main)",
            "echo \"$(git push origin main)\"",
            "echo `git push origin main`",
            "diff <(git push origin main) /dev/null",
            "bash <<'EOF'\ngit push origin main\nEOF",
            "bash <<< 'git push origin main'",
            "git -c alias.p=push p origin main",
            "watch -n 5 git push origin main",
            "git 'push' origin main",
            "git pu\"sh\" origin main",
            "git $\"push\" origin main",
            "git {push,origin,main}",
            "git push origin {main,claude/x} 2>/dev/null",
            "b=main; git push origin HEAD:$b",
            "export B=main && git push origin \"HEAD:${B}\"",
            "x=pu; y=sh; git $x$y origin main",
        ]:
            self.assertLevel("deny", cmd)

    def test_implicit_push_on_main_is_denied(self):
        git(self.repo, "switch", "-q", "main")
        for cmd in ["git push", "git push origin", "git push origin HEAD",
                    "git push origin @", "git push -u origin HEAD",
                    "git push --force-with-lease"]:
            self.assertLevel("deny", cmd)

    def test_push_default_upstream_follows_upstream_to_main(self):
        # claude/x tracks origin/main (as `worktree add -b ... origin/main` does).
        git(self.repo, "config", "push.default", "upstream")
        self.assertLevel("deny", "git push")
        self.assertLevel("deny", "git push origin claude/x")
        git(self.repo, "config", "push.default", "simple")
        self.assertLevel("none", "git push")
        self.assertLevel("none", "git push origin claude/x")

    def test_push_default_matching_is_denied(self):
        git(self.repo, "config", "push.default", "matching")
        self.assertLevel("deny", "git push")
        self.assertLevel("none", "git push origin claude/x")

    def test_repo_alias_for_push_is_resolved(self):
        git(self.repo, "config", "alias.pp", "push origin")
        git(self.repo, "config", "alias.sp", "!git push origin main")
        self.assertLevel("deny", "git pp main")
        self.assertLevel("none", "git pp claude/x")
        self.assertLevel("deny", "git sp")

    def test_script_files_are_read(self):
        script = os.path.join(self.repo, "p.sh")
        with open(script, "w") as fh:
            fh.write("#!/bin/sh\ngit push origin main\n")
        os.chmod(script, 0o755)
        self.assertLevel("deny", "bash p.sh")
        self.assertLevel("deny", "sh ./p.sh")
        self.assertLevel("deny", "./p.sh")
        self.assertLevel("deny", "source p.sh")


class ForceAndDelete(RepoCase):
    def test_plain_force_is_denied(self):
        for cmd in ["git push --force origin claude/x", "git push -f origin claude/x",
                    "git push -uf origin claude/x", "git push -fu origin claude/x",
                    "git push origin claude/x --force", "git push origin +claude/x",
                    "git push origin +HEAD:claude/x", "git push --forc origin claude/x"]:
            level = self.level(cmd)
            # `--forc` is ambiguous in git (force, force-with-lease, force-if-includes).
            self.assertIn(level, ("deny", "ask"), cmd)
            if cmd != "git push --forc origin claude/x":
                self.assertEqual("deny", level, cmd)

    def test_lease_outside_claude_asks(self):
        self.assertLevel("ask", "git push --force-with-lease origin HEAD:renovate/foo")
        self.assertLevel("ask", "git push origin claude/y --force-with-lease renovate/x")
        self.assertLevel("deny", "git push origin claude/y --force-with-lease HEAD:heads/main")
        self.assertLevel("none", "git push --force-with-lease origin claude/x")
        self.assertLevel("none", "git push --force-with-lease origin HEAD:claude/x")
        self.assertLevel("none", "git push --force-with-lease=claude/x:abc origin claude/x")
        self.assertLevel("none", "git push --force-with-lease")  # on claude/x

    def test_deletes_ask(self):
        for cmd in ["git push origin claude/x --delete", "git push origin claude/x -d",
                    "git push -d origin claude/x", "git push origin :claude/x",
                    "git push --del origin claude/x", "git push -ud origin claude/x"]:
            self.assertLevel("ask", cmd)

    def test_other_risky_options_ask(self):
        for cmd in ["git push --prune origin 'refs/heads/claude/*:refs/heads/claude/*'",
                    "git push --receive-pack='sh -c id' origin claude/x",
                    "git push --exec=x origin claude/x",
                    "git -c remote.origin.push=refs/heads/*:refs/heads/main push origin claude/x",
                    "echo main | xargs git push origin",
                    "find . -maxdepth 0 -exec git push origin {} \\;",
                    "echo 'git push origin main' | sh",
                    "git send-pack origin main",
                    "git push --frobnicate origin claude/x",
                    "git push origin \"$BRANCH\"",
                    "git push origin HEAD:$(git branch --show-current)",
                    "git $cmd origin claude/x",
                    "python3 -c \"import os; os.system('git push origin main')\"",
                    "python3 - <<'PY'\nimport subprocess\nsubprocess.run(['git', 'push', 'origin', 'main'])\nPY"]:
            self.assertLevel("ask", cmd)

    def test_remote_push_config_asks(self):
        git(self.repo, "config", "remote.origin.push", "refs/heads/claude/x:refs/heads/main")
        self.assertLevel("ask", "git push origin claude/x")

    def test_unresolvable_directory_asks(self):
        missing = os.path.join(self.tmp.name, "not-yet-cloned")
        self.assertLevel("ask", f"cd {missing} && git push")
        self.assertLevel("none", f"git -C {missing} push origin claude/x")


class Allowed(RepoCase):
    def test_routine_agent_pushes_have_no_opinion(self):
        for cmd in [
            "git push -u origin claude/x",
            "git push --set-upstream origin claude/x",
            "git push origin claude/x",
            "git push origin HEAD:claude/x",
            "git push origin HEAD",
            "git push",
            "git push -u origin claude/fix-main-docs",
            "git push origin claude/main-thing",
            "git push origin v1.0",
            "git push origin tag v1.0",
            "git push --tags origin",
            "git push --dry-run origin main",
            "git push -n origin main",
            "git -C " + self.repo + " push -u origin claude/x",
            "b=claude/x; git push -u origin $b",
        ]:
            self.assertLevel("none", cmd)

    def test_text_that_only_mentions_push_is_ignored(self):
        for cmd in [
            "git commit -m 'Deny git push origin main in hook'",
            "git commit -m \"$(cat <<'EOF'\nBlock git push origin main\nand don't push --force\nEOF\n)\"",
            "gh pr create --title t --body \"$(cat <<'EOF'\ngit push origin claude/x HEAD:heads/main\nEOF\n)\"",
            "cat > notes.md <<'EOF'\ngit push origin main\nEOF",
            "echo git push origin main",
            "grep -rn 'git push' .claude",
            "rg 'git push origin main' docs",
            "git log --grep='push origin main'",
            "git stash push -m wip",
            "pushd /tmp && popd",
            "cargo build --release",
            "just check",
        ]:
            self.assertLevel("none", cmd)

    def test_local_remotes_are_exempt(self):
        for cmd in ["git push scratch main", "git push scratch claude/x:main --force",
                    f"git push {self.origin} HEAD:main", "git push ../origin.git main",
                    f"git push file://{self.origin} main"]:
            self.assertLevel("none", cmd)
        self.assertLevel("ask", "git push scratch --receive-pack=x main")
        self.assertLevel("deny", "git push git@github.com:example/aios.git main")
        self.assertLevel("deny", "git -c url.x.insteadOf=y push scratch main")

    def test_url_rewriting_disables_the_exemption(self):
        git(self.repo, "config", "url.https://github.com/.insteadOf", self.tmp.name + "/")
        self.assertLevel("deny", "git push scratch main")

    def test_github_changes_ask(self):
        os.makedirs(os.path.join(self.repo, ".github", "workflows"))
        with open(os.path.join(self.repo, ".github", "workflows", "ci.yml"), "w") as fh:
            fh.write("on: push\n")
        git(self.repo, "add", ".github")
        git(self.repo, "commit", "-q", "-m", "ci")
        self.assertLevel("ask", "git push -u origin claude/x")
        git(self.repo, "push", "-q", "scratch", "claude/x")
        git(self.repo, "fetch", "-q", "scratch")
        guard.Repo._cache.clear()
        self.assertLevel("none", "git push -u origin claude/x")


class GitSemantics(RepoCase):
    """Cross-check the guard's model against what git really does."""

    def remote_main(self):
        return git(self.origin, "rev-parse", "refs/heads/main").strip()

    def test_heads_main_really_updates_main(self):
        before = self.remote_main()
        git(self.repo, "push", "-q", "scratch", "claude/x", "HEAD:heads/main")
        self.assertNotEqual(before, self.remote_main())

    def test_upstream_push_default_really_updates_main(self):
        before = self.remote_main()
        git(self.repo, "config", "push.default", "upstream")
        git(self.repo, "config", "branch.claude/x.remote", "scratch")
        git(self.repo, "push", "-q", "scratch", "claude/x")
        self.assertNotEqual(before, self.remote_main())


class HookProtocol(RepoCase):
    def run_hook(self, command, tool="Bash"):
        payload = {"tool_name": tool, "tool_input": {"command": command}, "cwd": self.repo,
                   "hook_event_name": "PreToolUse"}
        return subprocess.run([sys.executable, HOOK], input=json.dumps(payload),
                              capture_output=True, text=True, timeout=20)

    def test_deny_exits_2_with_json(self):
        proc = self.run_hook("git push origin claude/x HEAD:heads/main")
        self.assertEqual(2, proc.returncode)
        out = json.loads(proc.stdout)["hookSpecificOutput"]
        self.assertEqual("deny", out["permissionDecision"])
        self.assertIn("main", out["permissionDecisionReason"])

    def test_ask_exits_0_with_json(self):
        proc = self.run_hook("git push origin claude/x --delete")
        self.assertEqual(0, proc.returncode)
        self.assertEqual("ask", json.loads(proc.stdout)["hookSpecificOutput"]["permissionDecision"])

    def test_no_opinion_is_silent(self):
        for command, tool in [("git push -u origin claude/x", "Bash"), ("ls", "Bash"),
                              ("git push origin main", "Write")]:
            proc = self.run_hook(command, tool)
            self.assertEqual((0, ""), (proc.returncode, proc.stdout), command)

    def test_bad_input_is_silent(self):
        for data in ["", "not json", "[]", "{}"]:
            proc = subprocess.run([sys.executable, HOOK], input=data, capture_output=True,
                                  text=True, timeout=20)
            self.assertEqual((0, ""), (proc.returncode, proc.stdout), data)

    def test_unparseable_push_asks(self):
        proc = self.run_hook("git push origin 'main")
        self.assertEqual("ask", json.loads(proc.stdout)["hookSpecificOutput"]["permissionDecision"])


if __name__ == "__main__":
    unittest.main()
