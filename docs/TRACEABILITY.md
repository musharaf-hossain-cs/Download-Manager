# Requirement Traceability

This file defines the evidence format used as implementation proceeds. A requirement is not complete merely because it has a row; every `Verified` row must link to an automated test, review artifact, or reproducible command output.

## Status values

- `Unmapped`: no implementation slice owns the requirement yet.
- `Planned`: owning slice and intended evidence are known.
- `Implemented`: code exists but mandatory evidence is incomplete.
- `Verified`: all named evidence passes on the supported matrix.
- `Blocked`: a named, tracked condition prevents completion.

## Mapping template

| Requirement ID | Priority | Owning slice(s) | Status | Evidence | Notes |
|---|---|---|---|---|---|
| `EXAMPLE-001` | P0 | S000 | Planned | `path/to/test` or CI run URL | Replace this example when mappings begin. |
| `STATE-01` | P0 | S011–S012, S015 | Verified | `state::tests::transition_table_is_exhaustive`; `storage::tests::state_update_and_event_are_atomic` | Canonical transitions are reduced before an atomic state update + event insert. |
| `DATA-01` | P0 | S010 | Verified | `domain::tests::ids_serialize_as_stable_uuid_strings` | Jobs, segments, events, and requests use UUID-backed IDs. |
| `DATA-02` | P0 | S013–S016 | Verified | `storage::tests::database_creates_reopens_and_reports_schema_version`; `failed_migration_rolls_back_and_preserves_backup` | Migrations are ordered, transactional, and pre-migration backup protected. |
| `DATA-03` | P0 | S010, S053, S119 | Implemented | `JobMetadata` and SQLite migrations contain references/non-secret metadata only. | Encrypted Windows credential storage and request-context cleanup remain owned by later slices. |
| `DATA-05` | P0 | S017, S069 | Verified | `storage::tests::retention_never_age_deletes_active_or_partial_jobs` | Durable policy is verified; history UI remains S069. |
| `BI-012` | P0 | S019, S080 | Implemented | `ipc::tests::extension_rejects_oversize_invalid_urls_and_bad_nonces`; duplicate request-ID test | Version, strict fields, size, scheme, nonce, and replay checks exist; packaged-ID allowlist remains S080. |
| `SEC-002` | P0 | S018–S019 and feature slices | Implemented | UI/extension malformed, unknown-field, oversize, invalid-scheme, and invalid-nonce tests | Boundary foundation is verified; later page/media/header schemas retain ownership of their inputs. |

## Evidence rules

1. Use the exact stable ID from `REQUIREMENTS.md`.
2. Link the smallest automated test or artifact that proves the behavior.
3. Record each supported OS/browser variant when the requirement names a matrix.
4. A benchmark requirement links raw machine-readable results and the generated comparison report.
5. A security requirement links both the positive behavior and the rejection/abuse case.
6. Requirement changes follow section 20 of `REQUIREMENTS.md`; do not silently rewrite a mapping.

