#!/usr/bin/env python3
"""Rebuild Worker A's calls for the week from the exported ledger events.

One `ingest.batch_classified` event = one Worker A submit = one call. Its text is
what the runner sent: render_batch_text per batch (oldest first), then signal_text.
Writes calls.jsonl (kept on the host, never committed) and prints a summary.
"""
import json
import re
import sys
from collections import Counter

_NOISE_RE = re.compile(r"\[(?:Tool: [^\]]*|Tool result|TRUNCATED)\]")
_PREFIX_RE = re.compile(r"^\[(?:user|assistant|unknown)\]\s*")


def render_batch_text(payload):
    out = []
    for turn in payload.get("turns") or []:
        role = turn.get("role") or "unknown"
        text = turn.get("text") or ""
        if turn.get("truncated"):
            out.append(f"[{role}] {text} [TRUNCATED]\n")
        else:
            out.append(f"[{role}] {text}\n")
    return "".join(out)


def signal_text(batch_text):
    keep = []
    for line in batch_text.splitlines():
        if _NOISE_RE.sub("", _PREFIX_RE.sub("", line)).strip():
            keep.append(_NOISE_RE.sub("", line).rstrip())
    return "\n".join(keep)


def main(src, dst):
    batches = {}
    classified = []
    with open(src, encoding="utf-8") as fh:
        for line in fh:
            e = json.loads(line)
            p = e["payload"]
            if isinstance(p, str):
                p = json.loads(p)
            if e["event_type"] == "ingest.batch_received":
                batches[p["batch_id"]] = {
                    "text": signal_text(render_batch_text(p)),
                    "session_id": p.get("session_id", ""),
                    "actor_id": e["actor_id"],
                    "ts": e["ts"],
                }
            else:
                classified.append((e, p))
    models = Counter()
    rows, missing = [], 0
    for e, p in classified:
        ids = p.get("batch_ids") or ([p["batch_id"]] if p.get("batch_id") else [])
        models[p.get("classifier_model", "")] += 1
        texts, sess, actor = [], set(), ""
        for b in ids:
            info = batches.get(b)
            if info is None:
                missing += 1
                continue
            if info["text"]:
                texts.append(info["text"])
            sess.add(info["session_id"])
            actor = actor or info["actor_id"]
        caps = p.get("captures") or []
        rows.append({
            "event_id": e["event_id"],
            "ts": e["ts"],
            "model": p.get("classifier_model", ""),
            "role": (actor.rsplit(":", 1)[-1] if actor else ""),
            "sessions": sorted(sess),
            "n_batches": len(ids),
            "chars": sum(len(t) for t in texts),
            "text": "\n".join(texts),
            "kinds": [c.get("kind") for c in caps],
            "n_captures": len(caps),
        })
    with open(dst, "w", encoding="utf-8") as out:
        for r in rows:
            out.write(json.dumps(r) + "\n")
    print("classified events:", len(rows), "missing batch texts:", missing)
    print("models:", dict(models))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
