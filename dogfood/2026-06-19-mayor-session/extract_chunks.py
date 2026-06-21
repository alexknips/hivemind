#!/usr/bin/env python3
"""
Extract assistant text from the mayor session JSONL, grouped into
epoch chunks by bridge-session boundaries. Produces clean text chunks
suitable for dogfood classification.
"""
import json
import sys
from pathlib import Path

TRANSCRIPT = Path("/home/ubuntu/.claude/projects/-home-ubuntu-gc--gc-agents-mayor/9d852b7c-42a5-4e10-b196-46ad8484c214.jsonl")
OUT_DIR = Path("/tmp/hivemind-dogfood/chunks")
OUT_DIR.mkdir(parents=True, exist_ok=True)

MIN_CHUNK_CHARS = 1500  # merge epochs smaller than this with next

chunks = []
current_texts = []

def flush_epoch(texts, chunk_list):
    text = "\n\n".join(t.strip() for t in texts if t.strip())
    if text:
        chunk_list.append(text)

with open(TRANSCRIPT, encoding="utf-8") as f:
    for line in f:
        try:
            obj = json.loads(line)
        except Exception:
            continue

        t = obj.get("type", "")

        if t == "bridge-session":
            # Natural epoch boundary
            flush_epoch(current_texts, chunks)
            current_texts = []
        elif t == "assistant":
            msg = obj.get("message", {})
            content = msg.get("content", [])
            for c in content:
                if c.get("type") == "text":
                    text = c.get("text", "").strip()
                    if len(text) > 30:  # skip trivial one-liners
                        current_texts.append(text)

# flush final epoch
flush_epoch(current_texts, chunks)

# Merge tiny chunks with their successor
merged = []
buf = ""
for chunk in chunks:
    buf = (buf + "\n\n" + chunk).strip() if buf else chunk
    if len(buf) >= MIN_CHUNK_CHARS:
        merged.append(buf)
        buf = ""
if buf:
    merged.append(buf)

# Write chunks
for i, chunk in enumerate(merged):
    out = OUT_DIR / f"chunk_{i:03d}.txt"
    out.write_text(chunk)

print(f"Total raw epochs: {len(chunks)}")
print(f"Merged chunks (>={MIN_CHUNK_CHARS} chars): {len(merged)}")
for i, chunk in enumerate(merged):
    # Show first line of each chunk for orientation
    first_line = chunk.split("\n")[0][:100]
    print(f"  [{i:03d}] {len(chunk):6d} chars — {first_line}")
