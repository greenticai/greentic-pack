# PR-02-DOCS-SWEEP (greentic-pack): Remove legacy instructions from primary docs (canonical-only)

**Date:** 2026-02-19  
**Repo:** `greentic-pack`  
**Type:** Docs-only

## Why
Even with deprecation signals, legacy guidance in READMEs and quickstarts will keep causing drift.

This PR ensures primary docs teach the canonical v0.6 approach only.

## Goals
1) Primary docs (README, docs index, quickstarts) describe canonical v0.6 usage.
2) Legacy guidance is either:
   - deleted, or
   - moved to `docs/vision/legacy.md` and labeled legacy.

## Scope
### A) Audit docs
Codex must scan:
- README.md
- docs/**
- examples/**
- any ADR/RFC docs that read like current usage

Extend `docs/vision/legacy.md` with:
- “Docs moved here from README/docs because they describe legacy.”

### B) Rewrite primary docs
In README / docs index:
- Remove legacy setup flows, legacy manifests, legacy envelopes, legacy WIT worlds.
- Replace with canonical links:
  - `docs/vision/v0.6.md`
  - a short “Getting Started (v0.6)” section
  - a “Legacy/Compat” link

### C) Examples
If examples are legacy:
- either update the example to canonical (docs-only if possible), or
- mark as legacy with a banner and link to canonical replacement example.

## Acceptance criteria
- A new contributor following README cannot accidentally implement legacy patterns.
- Legacy content is clearly segregated and labeled.
- No code changes.

## Repo-specific notes (greentic-pack)
Flag as legacy:
- Any older manifest schema that competes with extensions/validators model
- Any docs that treat “packs as only providers” rather than apps/extensions
Canonical must emphasize:
- Packs as applications or platform extensions
- Extensions/validators contracts and metadata
- Signature/attestation hooks (even if not enforced yet)
