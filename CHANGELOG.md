# Changelog

## Unreleased

- Dev-channel binaries (`greentic-pack-dev`, installed by `gtc install --channel dev`)
  now report the version they ship as — `greentic-pack 1.2.<run-id>`, matching
  their `v1.2.<run-id>` release tag — instead of the develop base `1.2.0-dev.0`
  (greenticai/.github#262). A dev binary still reporting `1.2.0-dev.0` predates
  that fix; tools that gate on the reported version (greentic-designer's pack
  floor) judge such a build as outdated.

## 0.4.16

- Updated to `greentic-types` 0.4.15 and added support for the optional `dev_flows`
  field on `ComponentManifest`. `packc` preserves development-time flows when
  reading and writing manifests or `.gtpack` archives; no CLI changes required.
