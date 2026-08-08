#!/usr/bin/env python3
"""
Seed a project's existing memory/handoff docs into the TDAI memory gateway
under a given session key, so manager chat has continuity from day one
instead of starting cold. Generalized from the one-off
sportsball-bets-memory-backfill.py (git-agent-harness issue #884) so the
same import can be repeated for any project, including ones resurrected
long after this script was written.

Uses /capture, not /seed: /seed's pipeline writes into a disposable
per-call `seed-{timestamp}/` directory, never merging into the live store
that /recall and /capture actually query -- confirmed empirically against
the sportsball-bets backfill (l0_recorded came back non-zero per call, but
the live DB had zero rows for that session_key afterward).

Usage:
  backfill_project_docs.py --repo PATH --session-key KEY [--docs a.md,b.md]
                            [--dry-run] [--limit=N] [--gateway-url URL]

--docs is optional: without it, the script looks for a small set of
conventional filenames (MANAGER_MEMORY.md, MEMORY.md, in the repo root and
docs/) plus any docs/*handoff*.md (case-insensitive). Pass --docs to name
exact paths (relative to --repo) instead.
"""

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

DEFAULT_GATEWAY_URL = "http://127.0.0.1:8420"
API_KEY_FILE = Path.home() / ".config" / "gah" / "tdai-gateway.env"

CONVENTIONAL_DOCS = [
    "docs/MANAGER_MEMORY.md",
    "MANAGER_MEMORY.md",
    "MEMORY.md",
]


def api_key() -> str | None:
    try:
        for line in API_KEY_FILE.read_text().splitlines():
            if line.startswith("TDAI_GATEWAY_API_KEY="):
                return line.split("=", 1)[1].strip()
    except OSError:
        pass
    return None


def discover_docs(repo: Path) -> list[str]:
    found = [rel for rel in CONVENTIONAL_DOCS if (repo / rel).is_file()]
    docs_dir = repo / "docs"
    if docs_dir.is_dir():
        for path in sorted(docs_dir.glob("*.md")):
            if "handoff" in path.name.lower():
                rel = str(path.relative_to(repo))
                if rel not in found:
                    found.append(rel)
    return found


def capture_doc(gateway_url: str, session_key: str, project_label: str, relative_path: str, content: str) -> tuple[bool, str]:
    # Framed as a real Q&A turn, not a meta "import this file" instruction --
    # the L1 extractor summarizes conversational substance, and a synthetic
    # "import X" user_content produced a shallow, content-free summary
    # instead of digesting the actual doc (confirmed on the sportsball-bets
    # backfill's first pass).
    payload = {
        "user_content": f"What's the current state of the {project_label} project, per {relative_path}?",
        "assistant_content": content,
        "session_key": session_key,
    }
    headers = {"Content-Type": "application/json"}
    key = api_key()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    body = json.dumps(payload).encode()
    req = urllib.request.Request(f"{gateway_url}/capture", data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            result = json.loads(resp.read())
            return True, f"l0={result.get('l0_recorded')}"
    except (urllib.error.URLError, urllib.error.HTTPError, TimeoutError) as e:
        detail = e.read().decode(errors="replace") if isinstance(e, urllib.error.HTTPError) else str(e)
        return False, detail


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--repo", required=True, type=Path, help="Project checkout to read docs from")
    parser.add_argument("--session-key", required=True, help="e.g. gah:manager:github.com/org/repo")
    parser.add_argument("--docs", help="Comma-separated paths relative to --repo; default: auto-discover")
    parser.add_argument("--gateway-url", default=None, help=f"Default: $TDAI_GATEWAY_URL or {DEFAULT_GATEWAY_URL}")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--limit", type=int, default=None)
    args = parser.parse_args()

    gateway_url = args.gateway_url or os.environ.get("TDAI_GATEWAY_URL", DEFAULT_GATEWAY_URL)
    repo = args.repo.expanduser().resolve()
    project_label = repo.name

    rel_paths = [p.strip() for p in args.docs.split(",")] if args.docs else discover_docs(repo)

    docs = []
    for rel in rel_paths:
        path = repo / rel
        if not path.is_file():
            print(f"skip (missing): {rel}", file=sys.stderr)
            continue
        text = path.read_text(errors="replace").strip()
        if not text:
            print(f"skip (empty): {rel}", file=sys.stderr)
            continue
        docs.append((rel, text))

    if args.limit:
        docs = docs[: args.limit]

    print(f"Gateway: {gateway_url}")
    print(f"Session key: {args.session_key}")
    print(f"Docs to seed: {len(docs)}")
    print(f"Mode: {'DRY RUN' if args.dry_run else 'LIVE'}\n")

    if not docs:
        print("Nothing to seed (no matching docs found -- pass --docs to name them explicitly).", file=sys.stderr)
        sys.exit(1)

    ok_count = 0
    for rel, text in docs:
        if args.dry_run:
            print(f"[dry-run] {rel}: {len(text)} chars")
            ok_count += 1
            continue
        ok, detail = capture_doc(gateway_url, args.session_key, project_label, rel, text)
        print(f"{rel}: {'ok' if ok else 'FAILED'} ({detail})")
        if ok:
            ok_count += 1
        time.sleep(1.0)

    print(f"\nDone. seeded={ok_count}/{len(docs)}")


if __name__ == "__main__":
    main()
