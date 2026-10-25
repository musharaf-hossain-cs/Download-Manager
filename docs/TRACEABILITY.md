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
| `DL-004` | P0 | S021 | Verified | `probe::tests` | Redirects, final URL, length, range support, ETag, Last-Modified, MIME, disposition, and auth challenge are represented by the probe contract. |
| `DL-005` | P0 | S022–S023 | Verified | `filename::tests::filename_selection_prefers_rfc5987_then_legacy_url_and_mime`; sanitizer corpus | Unicode RFC 5987, quoted legacy values, URL/MIME fallback, traversal, ADS, device names, and controls are covered. |
| `DL-007` | P0 | S025, S070 | Implemented | `finalize::tests::same_volume_conflict_policies_are_explicit_and_safe` | Engine ask/overwrite/counter-rename/resume-if-same behavior is verified; persisted global default remains S070. |
| `STATE-02` | P0 | S027, S063–S064 | Implemented | pause transfer and atomic checkpoint tests | Engine pause acknowledgment is verified; the complete desktop action surface remains S063–S064. |
| `ENG-009` | P0 | S033–S034 | Implemented | `retry::tests` | Policy, jitter, budget, status mapping, and `Retry-After` parsing are verified; orchestration integration remains with later transfer controllers. |
| `RES-001` | P0 | S030, S040 | Implemented | `checkpoint::tests::fake_clock_and_byte_threshold_bound_checkpoint_writes` | Single-stream 5-second/8-MiB policy is verified; multi-segment persistence remains S040. |
| `RES-002` | P0 | S027 | Verified | `transfer::tests::pause_stops_the_stream_at_an_exact_synced_offset`; `storage::tests::graceful_pause_persists_exact_single_offset_atomically` | Network reads stop before the synced offset is atomically acknowledged. |
| `RES-003` | P0 | S031–S032 | Verified | `recovery::tests::twenty_seeded_process_kills_recover_without_corrupt_completion` | Twenty child processes are force-killed at seeded offsets; recovery discards at most one checkpoint window. |
| `RES-004` | P0 | S028 | Verified | `resume::tests::changed_etag_or_size_refuses_without_touching_partial` | Identity mismatch is rejected before the partial file is opened for append. |
| `RES-005` | P0 | S029 | Verified | `resume::tests::server_without_range_support_requires_restart_without_appending` | No-range resume returns a specific restart-required result and preserves bytes. |
| `RES-006` | P0 | S035–S036 | Verified | `integrity::tests` | Streaming cancellation and MD5/SHA-1/SHA-256/SHA-512 formats/mismatches pass; weak algorithms are compatibility-only. |
| `RES-007` | P0 | S025–S026 | Verified | `finalize::tests` | Same-volume rename and cross-volume copy/sync/SHA-256/rename preserve complete-file visibility. |
| `DATA-04` | P0 | S031 | Verified | `recovery::tests::database_ahead_behind_and_orphans_repair_safely` | DB-ahead, file-ahead, missing, and orphan partial states are detected or safely repaired. |
| `LIM-004` | P0 | S037, S115 | Implemented | `disk::tests` | Capacity precheck and exact safe disk-full offset are verified; automatic global active-job policy remains S115. |
| `AC-03` | P0 | S028 | Verified | changed ETag and length fixtures | Unsafe append is blocked without changing the partial. |
| `AC-12` | P0 | S037 | Implemented | precheck and injected disk-full tests | Engine result is actionable and byte-exact; desktop remediation flow remains later work. |
| `AC-13` | P0 | S023 | Verified | traversal/ADS/device/control/Unicode sanitizer corpus | Constructed output remains one component beneath the selected directory. |

## Evidence rules

1. Use the exact stable ID from `REQUIREMENTS.md`.
2. Link the smallest automated test or artifact that proves the behavior.
3. Record each supported OS/browser variant when the requirement names a matrix.
4. A benchmark requirement links raw machine-readable results and the generated comparison report.
5. A security requirement links both the positive behavior and the rejection/abuse case.
6. Requirement changes follow section 20 of `REQUIREMENTS.md`; do not silently rewrite a mapping.

