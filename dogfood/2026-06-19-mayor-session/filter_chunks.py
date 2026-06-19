#!/usr/bin/env python3
"""
Quick keyword filter to identify high-signal chunks (likely decisions)
vs noise (status narration, pass-done markers, monitoring).
"""
import json
from pathlib import Path

CHUNKS_DIR = Path("/tmp/hivemind-dogfood/chunks")

# Decision-signal indicators
SIGNAL_WORDS = [
    # Decision language
    "decided", "decision:", "locked", "confirmed", "approved", "settled",
    "will use", "will be", "we are going", "going with", "choosing",
    # Architecture/design
    "classifier", "ingest", "sidecar", "actor", "authorship", "layer",
    "capture", "ledger", "provenance", "compactif",
    # Action outcomes
    "merged", "dispatched", "created", "shipped", "fixed", "closed",
    "bead created", "slung", "unblocked", "nudged", "approved",
    # Scope/direction
    "not in scope", "out of scope", "defer", "defer to", "won't",
    "python long", "rust evaluator", "evaluator v0",
    # Explicit decision markers
    "design locked", "locked direction", "my lean:", "plan:",
    "approach:", "opt for", "go with", "key insight",
    "key finding", "important finding", "root cause",
    "fix:", "the fix:", "lesson:",
]

NOISE_WORDS = [
    "pass done", "next check", "loop re-armed", "re-arming",
    "loop fired", "pass done.", "check ~", "minutes",
]

results = []
for path in sorted(CHUNKS_DIR.glob("chunk_*.txt")):
    text = path.read_text().lower()
    lines = text.split("\n")
    first_line = lines[0][:120] if lines else ""

    signal_hits = [w for w in SIGNAL_WORDS if w in text]
    noise_hits = [w for w in NOISE_WORDS if w in text]

    # Score: signal strength minus noise discount
    score = len(signal_hits) * 2 - len(noise_hits)

    # Tag: high-signal (>=6), medium (3-5), noise (<3)
    if score >= 6:
        tag = "HIGH"
    elif score >= 3:
        tag = "MEDIUM"
    else:
        tag = "NOISE"

    results.append({
        "chunk": path.name,
        "chars": len(path.read_text()),
        "tag": tag,
        "score": score,
        "signal_hits": signal_hits[:8],  # top 8
        "first_line": first_line,
    })

# Print summary
for r in results:
    print(f"{r['chunk']} [{r['tag']:6s} {r['score']:+3d}] {r['chars']:6d}ch — {r['first_line'][:80]}")

print(f"\nSummary:")
print(f"  HIGH:   {sum(1 for r in results if r['tag']=='HIGH')}")
print(f"  MEDIUM: {sum(1 for r in results if r['tag']=='MEDIUM')}")
print(f"  NOISE:  {sum(1 for r in results if r['tag']=='NOISE')}")

# Write JSON for use by next step
Path("/tmp/hivemind-dogfood/chunk_tags.json").write_text(
    json.dumps(results, indent=2)
)
print("\nWrote /tmp/hivemind-dogfood/chunk_tags.json")
