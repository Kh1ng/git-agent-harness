#!/usr/bin/env python3
"""
Shared session-lifecycle hook for the TDAI memory gateway (tdai-memory-gateway,
127.0.0.1:8420), invoked by Claude Code, Codex, and Hermes hook configs.

One script, not one per tool: all three project-shape differs (stdin/stdout
contract, transcript location) but the actual gateway logic -- resolve a
durable project key from cwd, call /recall or /capture -- is identical. Each
tool's hook config passes --tool/--phase explicitly rather than sniffing env
vars, since the caller already knows which tool it is.

Session key scheme matches apps/server/src/managerChat/memoryGatewayClient.ts
in git-agent-harness: `gah:manager:{normalized-git-remote-url}`, keyed by the
project's git remote (survives repo/profile renames), not by tool or path.
Never blocks the calling tool on failure -- a broken hook must not break a
normal Claude Code / Codex / Hermes session, so every failure mode degrades
to "no context" / "capture skipped" with a note on stderr, never a non-zero
exit that could be mistaken for a blocking hook response.
"""

import argparse
import os
import json
import re
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

SETTINGS_FILE = Path(os.environ.get("GAH_MEMORY_HOOK_CONFIG", str(Path.home() / ".config/gah/memory-hooks.json")))
TDAI_API_KEY_FILE = Path.home() / ".config" / "gah" / "tdai-gateway.env"


def log(msg: str) -> None:
    print(f"[gah-memory-hook] {msg}", file=sys.stderr)


def api_key() -> str | None:
    if os.environ.get("TDAI_GATEWAY_API_KEY"):
        return os.environ["TDAI_GATEWAY_API_KEY"]
    try:
        for line in TDAI_API_KEY_FILE.read_text().splitlines():
            if line.startswith("TDAI_GATEWAY_API_KEY="):
                return line.split("=", 1)[1].strip()
    except OSError:
        pass
    return None


def normalize_remote_url(raw: str) -> str:
    """Mirrors memoryGatewayClient.ts's normalizeRemoteUrl exactly, so a
    project resolves to the same session key regardless of which tool
    (this hook, or the manager-chat panel) is asking."""
    s = raw.strip().lower()
    s = re.sub(r"^[a-z][a-z0-9+.\-]*://", "", s)
    s = re.sub(r"^[^@/]+@", "", s)
    s = re.sub(r":(?!\d+/)", "/", s, count=1)
    s = re.sub(r"\.git$", "", s)
    return s.rstrip("/")


def resolve_project_key(cwd: str) -> str:
    try:
        out = subprocess.run(
            ["git", "-C", cwd, "remote", "get-url", "origin"],
            capture_output=True, text=True, timeout=5,
        )
        if out.returncode == 0 and out.stdout.strip():
            return normalize_remote_url(out.stdout)
    except (OSError, subprocess.TimeoutExpired):
        pass
    # No git remote resolvable (not a repo, no origin) -- fall back to the
    # directory name rather than failing recall/capture outright.
    return Path(cwd).name or "unknown"


def session_key(cwd: str) -> str:
    return f"gah:manager:{resolve_project_key(cwd)}"


class NoGatewayRedirect(urllib.request.HTTPRedirectHandler):
    """Do not forward agent content or credentials to a redirected endpoint."""
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def gateway_post(path: str, body: dict) -> dict | None:
    settings = json.loads(SETTINGS_FILE.read_text()) if SETTINGS_FILE.exists() else {}
    base_url = os.environ.get("TDAI_GATEWAY_URL") or settings.get("gateway_url")
    if not base_url and TDAI_API_KEY_FILE.exists():
        base_url = "http://127.0.0.1:8420"  # Existing hand-configured installation.
    if not base_url:
        return None  # Fresh machine: no gateway yet, no network request.
    url = urlsplit(base_url)
    if url.scheme not in ('http', 'https') or not url.hostname or url.username or url.password or url.query or url.fragment:
        log('Invalid gateway URL; memory skipped')
        return None
    headers = {"Content-Type": "application/json"}
    key = api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    req = urllib.request.Request(
        f"{base_url.rstrip('/')}{path}",
        data=json.dumps(body).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.build_opener(NoGatewayRedirect()).open(req, timeout=8) as resp:
            return json.loads(resp.read())
    except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as e:
        log(f"{path} failed (continuing without it): {e}")
        return None


def last_turn_from_transcript(transcript_path: str) -> tuple[str, str] | None:
    """Extract the last user message + assistant reply from a Claude
    Code / Codex JSONL transcript (both use the same message-role shape).
    Returns None if nothing usable is found -- never raises."""
    if not transcript_path or not Path(transcript_path).exists():
        return None
    user_text = None
    assistant_text = None
    try:
        lines = Path(transcript_path).read_text().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = entry.get("message", entry)
        role = message.get("role")
        content = message.get("content")
        text = _flatten_content(content)
        if not text:
            continue
        if role == "assistant" and assistant_text is None:
            assistant_text = text
        elif role == "user" and user_text is None:
            user_text = text
        if user_text is not None and assistant_text is not None:
            break
    if user_text is None or assistant_text is None:
        return None
    return user_text, assistant_text


def _flatten_content(content) -> str:
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text":
                parts.append(block.get("text", ""))
        return "\n".join(p for p in parts if p)
    return ""


def do_recall(cwd: str, tool: str) -> str:
    key = session_key(cwd)
    result = gateway_post(
        "/recall",
        {"query": "recent project context and history", "session_key": key},
    )
    if not result or result.get("code") != 0:
        return ""
    return result.get("context", "") or ""


def do_capture(cwd: str, transcript_path: str) -> None:
    key = session_key(cwd)
    turn = last_turn_from_transcript(transcript_path)
    if turn is None:
        log("no usable turn found in transcript, skipping capture")
        return
    user_text, assistant_text = turn
    gateway_post(
        "/capture",
        {
            "user_content": user_text,
            "assistant_content": assistant_text,
            "session_key": key,
        },
    )


def do_flush(cwd: str) -> None:
    key = session_key(cwd)
    gateway_post("/session/end", {"session_key": key})


def emit_recall_output(tool: str, context: str) -> None:
    if not context:
        return
    if tool == "codex":
        # Codex mirrors Claude Code's hook event names but wants a JSON
        # envelope, not raw stdout -- confirmed against ponytail's own
        # working ponytail-runtime.js writeHookOutput() for isCodex.
        print(json.dumps({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": context,
            }
        }))
    elif tool == "hermes":
        print(json.dumps({"context": context}))
    else:  # claude
        print(context)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tool", required=True, choices=["claude", "codex", "hermes"])
    parser.add_argument("--phase", required=True, choices=["recall", "capture", "flush"])
    try:
        args = parser.parse_args()
    except SystemExit:
        return  # A hook usage error must not block a session.
    tool, phase = args.tool, args.phase

    try:
        payload = json.loads(sys.stdin.read() or "{}")
    except json.JSONDecodeError:
        payload = {}
    cwd = payload.get("cwd") or "."

    try:
        if phase == "recall":
            context = do_recall(cwd, tool)
            emit_recall_output(tool, context)
        elif phase == "capture":
            do_capture(cwd, payload.get("transcript_path", ""))
        elif phase == "flush":
            do_flush(cwd)
    except Exception as e:  # noqa: BLE001 -- a hook must never crash the caller
        log(f"unexpected error in phase={phase}: {e}")

    sys.exit(0)


if __name__ == "__main__":
    main()
