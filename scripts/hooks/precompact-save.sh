#!/usr/bin/env bash
# PreCompact hook: flush the Remember plugin's session memory before compaction.
#
# Compaction replaces the working context with a summary. The Remember plugin
# (remember@claude-plugins-official) saves on PostToolUse (cooldown-gated) and
# on SessionEnd, but registers no PreCompact hook, so whatever happened since
# its last save can be summarised away before it reaches .remember/now.md.
#
# This hook forwards the PreCompact payload to the plugin's own SessionEnd
# entry point (scripts/session-end-hook.sh). That script detaches itself and
# runs `save-session.sh <session_id> --force` in the background, which bypasses
# the cooldown and minimum-message gates and appends a summary of the
# unsaved transcript tail to now.md. It does not write remember.md (the
# first-person handoff that only /remember can compose).
#
# Contract: never blocks or fails compaction. Every path exits 0, the plugin
# hook detaches immediately, and this script returns in well under a second.
# It is a no-op when python3 is missing, when the plugin is not installed for
# this project, or when it is disabled via enabledPlugins.
#
# Input (stdin): the PreCompact hook JSON (session_id, transcript_path, cwd,
# hook_event_name, compaction_trigger or trigger, ...).

set -u

# The plugin's Haiku summariser runs a nested `claude`; never recurse into it.
[ -n "${REMEMBER_NESTED_SUMMARIZER:-}" ] && exit 0
command -v python3 >/dev/null 2>&1 || exit 0

payload=""
[ -t 0 ] || payload="$(cat)"

# Resolve the active plugin install and tag the payload with a reason the
# plugin logs (it validates `reason` against [A-Za-z0-9_]).
resolved="$(PRECOMPACT_PAYLOAD="$payload" python3 - <<'PY' 2>/dev/null
import glob
import json
import os
import re

home = os.path.expanduser("~")
config_dir = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.join(home, ".claude")
plugins_dir = os.path.join(config_dir, "plugins")
project_dir = os.environ.get("CLAUDE_PROJECT_DIR", "")
entry = os.path.join("scripts", "session-end-hook.sh")


def load(path):
    try:
        with open(path) as fh:
            return json.load(fh)
    except (OSError, ValueError):
        return None


def is_remember(key):
    return isinstance(key, str) and key.split("@", 1)[0] == "remember"


# enabledPlugins, merged user < project < local (later scopes win).
enabled = {}
settings_files = [os.path.join(config_dir, "settings.json")]
if project_dir:
    settings_files += [
        os.path.join(project_dir, ".claude", "settings.json"),
        os.path.join(project_dir, ".claude", "settings.local.json"),
    ]
for path in settings_files:
    data = load(path)
    if isinstance(data, dict) and isinstance(data.get("enabledPlugins"), dict):
        for key, value in data["enabledPlugins"].items():
            if is_remember(key):
                enabled[key] = bool(value)

root = None
registry = load(os.path.join(plugins_dir, "installed_plugins.json"))
if isinstance(registry, dict):
    plugins = registry.get("plugins", registry)
    for key, installs in (plugins.items() if isinstance(plugins, dict) else []):
        if not is_remember(key) or not enabled.get(key, False):
            continue
        for inst in installs if isinstance(installs, list) else [installs]:
            if not isinstance(inst, dict):
                continue
            scope = inst.get("scope", "user")
            if scope != "user" and inst.get("projectPath") != project_dir:
                continue
            path = inst.get("installPath") or ""
            if os.path.isfile(os.path.join(path, entry)):
                root = path
                break
        if root:
            break
elif registry is None and any(enabled.values()):
    # Registry unreadable: fall back to the newest cached version.
    def version_key(path):
        name = os.path.basename(path)
        return [int(p) if p.isdigit() else -1 for p in re.split(r"[.+-]", name)]

    candidates = [
        os.path.dirname(os.path.dirname(p))
        for p in glob.glob(os.path.join(plugins_dir, "cache", "*", "remember", "*", entry))
    ]
    if candidates:
        root = max(candidates, key=version_key)

if not root:
    raise SystemExit(1)

try:
    data = json.loads(os.environ.get("PRECOMPACT_PAYLOAD") or "{}")
except ValueError:
    data = {}
if not isinstance(data, dict):
    data = {}
trigger = str(data.get("compaction_trigger") or data.get("trigger") or "unknown")
data["reason"] = "precompact_" + re.sub(r"[^A-Za-z0-9_]", "_", trigger)
print(root)
print(json.dumps(data, separators=(",", ":")))
PY
)" || exit 0

plugin_root="${resolved%%$'\n'*}"
forward="${resolved#*$'\n'}"
[ -n "$plugin_root" ] && [ "$forward" != "$resolved" ] || exit 0

printf '%s\n' "$forward" |
    env -u PLUGIN_ROOT -u REMEMBER_SESSION_END_FOREGROUND \
        CLAUDE_PLUGIN_ROOT="$plugin_root" \
        bash "$plugin_root/scripts/session-end-hook.sh" >/dev/null 2>&1 || true

exit 0
