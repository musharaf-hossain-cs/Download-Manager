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
| `DATA-03` | P0 | S010, S053, S119 | Implemented | `credentials::tests::windows_store_round_trips_without_plaintext_database_or_log_value` | Windows secrets are user-vault encrypted and SQLite/log output contains references only; request-context expiry cleanup remains S119. |
| `DATA-05` | P0 | S017, S069 | Verified | `storage::tests::retention_never_age_deletes_active_or_partial_jobs`; desktop `history.test.ts` retention cases | Durable cleanup and the completed-only history retention UI never age-delete active or partial jobs. |
| `BI-012` | P0 | S019, S080 | Implemented | `ipc::tests::extension_rejects_oversize_invalid_urls_and_bad_nonces`; duplicate request-ID test | Version, strict fields, size, scheme, nonce, and replay checks exist; packaged-ID allowlist remains S080. |
| `SEC-002` | P0 | S018–S019 and feature slices | Implemented | UI/extension malformed, unknown-field, oversize, invalid-scheme, and invalid-nonce tests | Boundary foundation is verified; later page/media/header schemas retain ownership of their inputs. |
| `BROW-04` | P1 | S082 | Verified | extension `manifest.test.ts` MV3 and content-security-policy checks | The Chromium shell uses Manifest V3 and permits packaged extension code only. |
| `SEC-004` | P0 | S082 | Implemented | extension `manifest.test.ts` least-permission check | The shell requests only `nativeMessaging`, with no host permissions, content scripts, background worker, or web-accessible resources; later features must justify additions. |
| `DL-004` | P0 | S021 | Verified | `probe::tests` | Redirects, final URL, length, range support, ETag, Last-Modified, MIME, disposition, and auth challenge are represented by the probe contract. |
| `DL-005` | P0 | S022–S023 | Verified | `filename::tests::filename_selection_prefers_rfc5987_then_legacy_url_and_mime`; sanitizer corpus | Unicode RFC 5987, quoted legacy values, URL/MIME fallback, traversal, ADS, device names, and controls are covered. |
| `DL-006` | P0 | S061–S062, S067 | Verified | desktop `add-url-dialog.test.ts` and `categories.test.ts`; engine `desktop::tests::url_probe_*` and `start_choices_persist_exactly_one_job_in_the_requested_state` | URL, Unicode filename, writable destination, MIME, size preview, category defaults, and durable start timing are verified. |
| `DL-007` | P0 | S025, S070 | Implemented | `finalize::tests::same_volume_conflict_policies_are_explicit_and_safe` | Engine ask/overwrite/counter-rename/resume-if-same behavior is verified; persisted global default remains S070. |
| `STATE-02` | P0 | S027, S063–S064, S076 | Implemented | pause transfer/checkpoint tests; `desktop::tests::start_pause_resume_and_stop_controls_are_atomic_and_journaled`; S064 cancellation tests | Start, pause, resume, stop, cancel, and remove controls are acknowledged; open-file/folder actions remain S076. |
| `STATE-03` | P0 | S064 | Verified | `desktop::tests::cancel_and_record_only_removal_preserve_the_exact_local_file`; `delete_file_requires_confirmation_for_the_exact_job_target`; desktop confirmation-binding test | Record-only removal is the pathless default; file deletion requires the exact engine-derived final/partial path, and cancellation preserves both. |
| `STATE-04` | P0 | S065 | Verified | desktop `download-list.test.ts` mixed-state bulk tests | Resume and stop target only eligible selected states; Delete Completed removes records only and continues after individual failures. |
| `STATE-05` | P0 | S063 | Verified | desktop `download-list.test.ts` optimistic rejection test; engine invalid-action durability test | The UI previews the legal target state, then applies the acknowledged state or restores the prior state without a durable event on rejection. |
| `STATE-06` | P0 | S062 | Verified | `desktop::tests::duplicate_choices_focus_add_or_atomically_replace_one_durable_job`; desktop `add-url-dialog.test.ts` | Focus preserves the existing job, add creates exactly one additional record, and replacement is transactional. |
| `ENG-009` | P0 | S033–S034 | Implemented | `retry::tests` | Policy, jitter, budget, status mapping, and `Retry-After` parsing are verified; orchestration integration remains with later transfer controllers. |
| `RES-001` | P0 | S030, S040 | Verified | checkpoint fake-clock test; `segmented::tests::restart_requests_only_persisted_missing_segment_suffixes` | Periodic checkpoints and durable per-segment offsets resume only missing suffixes. |
| `RES-002` | P0 | S027 | Verified | `transfer::tests::pause_stops_the_stream_at_an_exact_synced_offset`; `storage::tests::graceful_pause_persists_exact_single_offset_atomically` | Network reads stop before the synced offset is atomically acknowledged. |
| `RES-003` | P0 | S031–S032 | Verified | `recovery::tests::twenty_seeded_process_kills_recover_without_corrupt_completion` | Twenty child processes are force-killed at seeded offsets; recovery discards at most one checkpoint window. |
| `RES-004` | P0 | S028 | Verified | `resume::tests::changed_etag_or_size_refuses_without_touching_partial` | Identity mismatch is rejected before the partial file is opened for append. |
| `RES-005` | P0 | S029 | Verified | `resume::tests::server_without_range_support_requires_restart_without_appending` | No-range resume returns a specific restart-required result and preserves bytes. |
| `RES-006` | P0 | S035–S036 | Verified | `integrity::tests` | Streaming cancellation and MD5/SHA-1/SHA-256/SHA-512 formats/mismatches pass; weak algorithms are compatibility-only. |
| `RES-007` | P0 | S025–S026 | Verified | `finalize::tests` | Same-volume rename and cross-volume copy/sync/SHA-256/rename preserve complete-file visibility. |
| `RES-008` | P0 | S038, S041 | Verified | delayed two-range hash test; invalid/ignored `Content-Range` fallback test | Adjacent range coverage is exact, and inconsistent range responses cannot corrupt output. |
| `ENG-002` | P0 | S039, S045–S046 | Verified | fixed planner boundary/property test; adaptive ramp/backoff tests | Fixed configuration and Auto remain bounded to 1–32 connections. |
| `ENG-003` | P0 | S042–S043 | Verified | slow-worker stealing and minimum-split tests | Idle workers split the largest active remainder without tiny-segment explosion. |
| `ENG-004` | P0 | S044 | Verified | keep-alive fixture reports one handshake and one reused request | Connection identity metrics distinguish handshakes from pooled reuse. |
| `ENG-005` | P0 | S045–S046, S049 | Implemented | shaped ramp, pressure backoff, and weighted fairness tests | Throughput/retry/throttle/disk/fairness behavior is verified; later benchmark campaigns provide measured tuning evidence. |
| `ENG-006` | P0 | S041 | Verified | `segmented::tests::ignored_or_invalid_content_range_falls_back_without_corruption` | Positional output is discarded before a clean single-stream restart. |
| `ENG-007` | P0 | S048 | Verified | `limits::tests::global_and_per_host_caps_never_exceed_under_many_jobs` | Owned permits enforce global and normalized per-host ceilings. |
| `ENG-010` | P0 | S052–S053 | Implemented | bounded Basic/Digest loop and Windows vault round-trip tests | Basic/Digest and encrypted credentials are verified; OS-integrated enterprise schemes remain capability-dependent. |
| `ENG-011` | P0 | S047 | Verified | virtual-time global/per-job token-bucket test | Aggregate and per-download rates remain within ±5%. |
| `ENG-012` | P0 | S049 | Verified | `fairness::tests::priority_has_weighted_advantage_without_starvation` | High/normal/low service is 4:2:1 with bounded low-priority wait. |
| `ENG-016` | P0 | S050 | Verified | forced-retry byte accounting test | Unique coverage, wire traffic, and retry overhead are reported separately. |
| `ENG-018` | P1 | S054 | Verified | dual-stack fallback fixture | Resolver-ordered IPv6/IPv4 addresses receive bounded attempts and safe fallback. |
| `NET-001` | P0 | S051 | Verified | local proxy routing and no-proxy wildcard matrix | System, direct, per-scheme manual proxy, and host-scoped bypass modes are represented. |
| `NET-003` | P0 | S052 | Implemented | Basic/Digest success, rejection, and request-count assertions | Basic and Digest are verified; enterprise schemes remain conditional on later OS-stack integration. |
| `NET-004` | P0 | S053 | Verified | Windows Credential Manager round-trip/delete and plaintext scan | Only the current user can retrieve the OS-vault secret; persistence/logging uses a reference. |
| `LIM-001` | P0 | S047 | Verified | virtual-time rate assertion | Global and per-download token buckets are both enforced. |
| `LIM-003` | P0 | S048 | Verified | many-job global/per-host permit test | Active connection admission never exceeds configured aggregate or host caps. |
| `DATA-04` | P0 | S031 | Verified | `recovery::tests::database_ahead_behind_and_orphans_repair_safely` | DB-ahead, file-ahead, missing, and orphan partial states are detected or safely repaired. |
| `LIM-004` | P0 | S037, S115 | Implemented | `disk::tests` | Capacity precheck and exact safe disk-full offset are verified; automatic global active-job policy remains S115. |
| `AC-03` | P0 | S028 | Verified | changed ETag and length fixtures | Unsafe append is blocked without changing the partial. |
| `AC-12` | P0 | S037 | Implemented | precheck and injected disk-full tests | Engine result is actionable and byte-exact; desktop remediation flow remains later work. |
| `AC-13` | P0 | S023 | Verified | traversal/ADS/device/control/Unicode sanitizer corpus | Constructed output remains one component beneath the selected directory. |
| `ORG-001` | P0 | S067 | Implemented | desktop `categories.test.ts` deterministic defaults | The six required defaults and stable identifiers exist; editing/add/remove remains later category-management work. |
| `ORG-002` | P0 | S068 | Implemented | desktop `categories.test.ts` ordered precedence table and fallback case | Extension, MIME, and source-host rules use visible first-match priority with General fallback; optional size rules remain later work. |
| `ORG-003` | P0 | S067 | Verified | desktop `categories.test.ts` destination policy cases | Each category has a deterministic destination and an explicit remembered-last-folder policy. |
| `ORG-004` | P0 | S058, S060, S066 | Verified | desktop `download-list.test.ts`; `virtual-list.test.ts`; `segment-detail.test.ts` fake snapshots | Persisted list metadata, bounded large-list projection, and per-segment/aggregate progress, speed, and ETA detail are verified. |
| `ORG-005` | P0 | S060, S069 | Implemented | desktop `virtual-list.test.ts` stable sort and 10,000-row budget; `history.test.ts` search/filter cases | Sorting plus completed-history full-text/category/date filtering are verified; grouping and saved views remain later work. |
| `ORG-006` | P0 | S069 | Verified | desktop `history.test.ts` clear and retention cases | Retention and manual clearing operate through a record-only port and leave downloaded files untouched. |
| `OPS-001` | P0 | S070 | Verified | desktop `settings.test.ts` search, section/all reset, rollback, and isolation cases | Settings are searchable and resettable per section or globally through a preference-only persistence port that cannot address jobs or files. |

## Evidence rules

1. Use the exact stable ID from `REQUIREMENTS.md`.
2. Link the smallest automated test or artifact that proves the behavior.
3. Record each supported OS/browser variant when the requirement names a matrix.
4. A benchmark requirement links raw machine-readable results and the generated comparison report.
5. A security requirement links both the positive behavior and the rejection/abuse case.
6. Requirement changes follow section 20 of `REQUIREMENTS.md`; do not silently rewrite a mapping.

