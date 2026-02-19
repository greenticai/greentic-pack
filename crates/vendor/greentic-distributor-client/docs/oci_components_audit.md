# OCI Components Audit

This audit documents what `greentic-distributor-client` supports around OCI/GHCR components and what changed in this PR. References use workspace-relative paths.

## Baseline before this PR
- Client surfaces OCI references only as data: `ArtifactLocation::OciReference` is a DTO from `greentic-types` with no resolver logic (`src/types.rs`, `src/wit_client.rs` conversion around `artifact_location.kind`).
- No OCI registry client, caching layer, or digest enforcement existed. The only distributors were:
  - WIT bindings calling distributor-api imports (`src/wit_client.rs`).
  - HTTP JSON runtime client (`src/http.rs`).
  - Dev-only filesystem `DistributorSource` (`greentic-distributor-dev/src/lib.rs`).
- No signature/provenance verification is implemented; signatures are only carried through the DTO as `SignatureSummary`.

## What is supported now
- **OCI component resolution (anonymous HTTPS, GHCR-friendly):** `src/oci_components.rs` (feature `oci-components`) introduces `OciComponentResolver` using `oci-distribution` with HTTPS enforced.
- **Extension model:** `ComponentsExtension` (`src/oci_components.rs`) represents `extensions.greentic.components` with `refs` and `mode` (`eager`/`lazy`).
- **Digest policy:** Tagged refs are rejected unless `ComponentResolveOptions.allow_tags` is set; digest pins are enforced and mismatches error out.
- **Cache & offline:** Content-addressed cache at `${GREENTIC_HOME:-$HOME/.greentic}/cache/oci/<sha256>/component.wasm` plus `metadata.json`; offline mode requires a cache hit (`src/oci_components.rs`).
- **Accepted artifact shapes:** Pulls OCI image or artifact manifests; prefers WASM media types (`application/vnd.wasm.component.v1+wasm`, `application/vnd.module.wasm.content.layer.v1+wasm`) and falls back to the first layer.
- **Tests:** `tests/oci_components.rs` exercises digest-pinned fetch/caching, tag rejection, offline behavior, tag opt-in, and invalid reference errors with a mock registry client. Cache metadata now records the manifest digest to aid future verification work.

## Answers to audit questions
1. **Can the client pull packs from OCI?** Yes, a separate pack fetcher exists under feature `pack-fetch` (`src/oci_packs.rs`) with anonymous HTTPS pulls and caching. Component resolution remains in `src/oci_components.rs`.
2. **Artifact types understood today?** Previously none; now OCI image/artifact manifests with WASM-oriented layers are handled via `oci-distribution` (anonymous HTTPS).
3. **Caching?** Previously none. Now content-addressed cache in `cache/oci/<sha256>/component.wasm` with metadata (`src/oci_components.rs`).
4. **Digest refs enforced?** Previously not. Now digest pins are required by default; tag refs error unless `allow_tags` is enabled, and digest mismatches fail (`OciComponentError::DigestMismatch`).
5. **Component notion separate from pack?** The library still treats components only via distributor APIs/DTOs. The new extension model (`ComponentsExtension`) is independent of pack schema and does not alter core types.
6. **Signature/provenance verification?** None. `SignatureSummary` is propagated but not verified (no keys/verification path implemented).
