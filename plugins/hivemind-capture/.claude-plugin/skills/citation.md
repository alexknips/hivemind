---
name: citation
description: Pin version references when a URL appears in a HiveMind decision capture. Trigger when any URL is present and a capture is being written. Guides the agent to resolve and record the version-at-citation before capturing.
---

# HiveMind Citation Skill

When a URL appears in the context of a decision, evidence, or hypothesis being
captured, you MUST pin the version the citer saw. A bare URL is not durable
evidence — the document can change; the citation must preserve what was true
at capture time.

## Trigger conditions

Apply this skill when ALL of the following hold:
1. A URL is present in the capture context (Google Doc, Confluence page, GitHub
   file link, or any web URL cited as evidence or rationale).
2. You are about to call `/capture` or `hivemind emit` or the MCP capture tool.

## Pinning by source type

### Git-tracked files (github.com/…/blob/…, local path)

Resolve to a commit hash before capturing:

```bash
# For a GitHub blob URL: derive the commit SHA from the URL or via git
git rev-parse HEAD  # if the file is in the current repo
# For an external repo: note the commit hash from the URL (github.com/owner/repo/blob/<sha>/path)
```

Include in the `--rationale` or a separate `--kind evidence` capture:
```
Source: <original URL> (commit <sha>, <YYYY-MM-DD>)
```

### Google Docs (docs.google.com/document/d/…)

Use the revision ID from the URL if present (`/edit#heading=h.xxx&rev=N`), or
note the document's "Last edited" timestamp shown in the document's title bar or
`File > Version history`. Record it as:
```
Source: <original URL> (Google Doc, last-edited <YYYY-MM-DD>, title: <doc title>)
```

### Confluence (*.atlassian.net/wiki/…)

Note the page version from the URL (`/versions/N`) or the "Last updated" date
visible on the page. Record as:
```
Source: <original URL> (Confluence, version <N> or <YYYY-MM-DD>, title: <page title>)
```

### Plain web URLs (neither Git nor SaaS doc)

For general web pages, capture the access date:
```
Source: <original URL> (accessed <YYYY-MM-DD>)
```

Note: web pages may change without versioning. Mark these as lower-confidence
evidence (`extraction_confidence` ≤ 0.7) unless the page is archival or versioned.

## Capture form with citation

After resolving the version, include it in the rationale of the capture:

```text
/capture "Key finding from the design doc" --kind evidence \
  --rationale "The design document at <URL> (commit abc1234, 2026-07-16) states that ..."
```

Or as a separate evidence capture referencing the source:

```text
/capture "Evidence: connector design decisions" --kind evidence \
  --rationale "From docs/INGESTION_CONNECTORS.md at commit abc1234 (2026-07-16): ..."
```

## What NOT to do

- Do not capture a bare URL without version resolution.
- Do not make up a commit hash or revision number.
- If the version cannot be resolved (no hash in URL, no date visible), state
  `(version unknown, accessed <date>)` rather than omitting the citation metadata.
- Do not add citation metadata to non-capture tool calls or general chat.

## Future enhancement

Once the ld68 connector layer is merged, `hivemind import connector --url <url>
--max-versions 1` will handle version pinning automatically and create a proper
evidence node with full source provenance. Until then, use the manual citation
approach above.
