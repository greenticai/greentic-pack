# Migration status — secrets move into greentic-types
- What changed: Distributor resolve responses and pack-status-v2 now surface optional `secret_requirements` using the canonical `SecretRequirement` from `greentic-types`; dependencies pinned to `greentic-interfaces-guest >= 0.4.65` and `greentic-types >= 0.4.23`.
- Current status: Complete — WIT and HTTP clients thread through `secret_requirements` (None when older distributors omit the field).
- Next steps:
  - Consumers should read `secret_requirements` when present and prefer `get_pack_status_v2` for structured status.
  - Run `greentic-secrets init --pack ...` ahead of time when requirements are returned.
