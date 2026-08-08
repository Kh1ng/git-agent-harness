#!/usr/bin/env python3
"""One-off analysis (2026-08-08): backend/model success rate by ticket
difficulty, read directly from the ledger. A real, permanent version of
this cross-tab is tracked as issue #907 (gah report --group-by
difficulty); this script is the throwaway version that produced the
findings referenced there and in docs/analysis/model-vs-complexity-2026-08-08.txt.

Usage: python3 scripts/analysis/model_vs_complexity.py [path/to/ledger.jsonl] [profile]
Defaults to $GAH_ARTIFACT_ROOT/ledger.jsonl (or ~/workspace/agent-lab/artifacts
as a last resort) and profile "gah".
"""

import json
import os
import sys
from collections import defaultdict

default_root = os.environ.get(
    "GAH_ARTIFACT_ROOT", os.path.expanduser("~/workspace/agent-lab/artifacts")
)
path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(default_root, "ledger.jsonl")
profile = sys.argv[2] if len(sys.argv) > 2 else "gah"

rows = []
with open(path) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            d = json.loads(line)
        except Exception:
            continue
        if d.get("profile") != profile:
            continue
        if d.get("mode") not in ("improve", "fix"):
            continue
        rows.append(d)

# key: (backend, difficulty) -> counts
stats = defaultdict(
    lambda: {
        "attempts": 0,
        "validation_pass": 0,
        "agent_no_progress": 0,
        "needs_fix": 0,
        "input_tokens": 0,
        "output_tokens": 0,
    }
)

for d in rows:
    backend = d.get("effective_backend") or d.get("backend") or "unknown"
    difficulty = d.get("difficulty") or "unlabeled"
    key = (backend, difficulty)
    s = stats[key]
    s["attempts"] += 1
    if d.get("validation_result") == "passed":
        s["validation_pass"] += 1
    if d.get("failure_class") == "agent_no_progress":
        s["agent_no_progress"] += 1
    if d.get("review_verdict") == "NEEDS_FIX":
        s["needs_fix"] += 1
    usage = d.get("usage") or {}
    s["input_tokens"] += usage.get("input_tokens") or 0
    s["output_tokens"] += usage.get("output_tokens") or 0

print(
    f"{'backend':<12} {'difficulty':<10} {'attempts':>8} {'val_pass':>9} "
    f"{'no_progress':>11} {'needs_fix':>10} {'success%':>9} {'input_tok':>12} {'output_tok':>11}"
)
print("-" * 100)
for (backend, difficulty), s in sorted(stats.items(), key=lambda x: (-x[1]["attempts"])):
    success_pct = (s["validation_pass"] / s["attempts"] * 100) if s["attempts"] else 0
    print(
        f"{backend:<12} {difficulty:<10} {s['attempts']:>8} {s['validation_pass']:>9} "
        f"{s['agent_no_progress']:>11} {s['needs_fix']:>10} {success_pct:>8.1f}% "
        f"{s['input_tokens']:>12,} {s['output_tokens']:>11,}"
    )

print()
print(f"Total improve/fix dispatch entries analyzed: {len(rows)}")
