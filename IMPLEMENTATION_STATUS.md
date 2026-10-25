# Implementation Status

- Last updated: 2026-10-26
- Current milestone: Milestone 3 — Segmented high-speed engine (next slice: S038)
- Status authority: `PROGRESSIVE_IMPLEMENTATION_PLAN.md`
- Traceability format: `docs/TRACEABILITY.md`

A slice is marked `DONE` only after its focused checks and all established fast regression checks pass. Long-running or externally hosted evidence is linked in the evidence cell when applicable.

| Slice | Status | Evidence |
|---|---|---|
| S000 | DONE | Baseline requirements, plan, README, and hygiene files in commit `31d4a56`. |
| S001 | DONE | Toolchains/workspaces pinned in `5bc2fb5`; clean checkout validated by Baseline CI. |
| S002 | DONE | Rust engine/CLI/native-host and version snapshots in `20be380`; Cargo tests green. |
| S003 | DONE | Desktop/extension packages and smoke tests in `08ffdfe`; pnpm frozen install and tests green. |
| S004 | DONE | Accepted process-boundary/dependency decision in `docs/adr/0001-application-architecture.md`; both stacks build. |
| S005 | DONE | Ledger and traceability template in `12e1af4`; verifier covers all 183 slices. |
| S006 | DONE | Rust format/clippy/test gate in `59bab8e`; seeded formatting failure rejected and restored. |
| S007 | DONE | TypeScript format/lint/build/type/test gate in `52cca64`; seeded formatting failure rejected and restored. |
| S008 | DONE | Windows [Baseline CI run 40199752939](https://github.com/musharaf-hossain-cs/Download-Manager/actions/runs/40199752939) passed all three jobs. |
| S009 | DONE | [Security policy run 40199752905](https://github.com/musharaf-hossain-cs/Download-Manager/actions/runs/40199752905) passed advisories, licenses, sources, and full-history secret scan. |
| S010 | DONE | `domain::tests`: stable UUID serialization, URL/timestamp/range invalid-value rejection. |
| S011 | DONE | `state::tests::state_event_json_is_stable`; canonical state/event JSON snapshot. |
| S012 | DONE | `state::tests::transition_table_is_exhaustive`; every state pair accepted or rejected by the reducer table. |
| S013 | DONE | `storage::tests::database_creates_reopens_and_reports_schema_version`; configured SQLite create/reopen at schema v2. |
| S014 | DONE | `storage::tests::job_and_remote_identity_crud_round_trip` and `job_and_identity_insert_rolls_back_as_one_transaction`. |
| S015 | DONE | `storage::tests::segments_enforce_ranges_and_events_are_ordered`; SQL constraints and durable sequence ordering. |
| S016 | DONE | `storage::tests::failed_migration_rolls_back_and_preserves_backup`; failed DDL rolls back and backup/original remain usable. |
| S017 | DONE | `storage::tests::retention_never_age_deletes_active_or_partial_jobs`; only completed history is age-deleted. |
| S018 | DONE | Rust `ipc::tests::ui_schema_has_a_stable_snapshot` + rejection tests and desktop `protocol.test.ts` cross-language snapshot. |
| S019 | DONE | Rust oversize/URL/nonce/replay rejection tests and extension `protocol.test.ts` encoding/validation tests. |
| S020 | DONE | `test_support::tests::programmable_server_serves_status_headers_body_and_delay`; local fixture controls status, headers, body, delay, and captures requests. |
| S021 | DONE | `probe::tests`: HEAD denial falls back to a one-byte ranged GET; redirects, unknown length, range identity, and failure status are covered. |
| S022 | DONE | `filename::tests::filename_selection_prefers_rfc5987_then_legacy_url_and_mime`; RFC 5987 Unicode, quoted legacy, URL, and MIME fallback table passes. |
| S023 | DONE | `filename::tests::sanitizer_rejects_traversal_ads_devices_and_control_characters` plus Unicode length-bound coverage. |
| S024 | DONE | `transfer::tests`: known/unknown-length HTTP bodies stream to a synced temp file with monotonic progress and verified SHA-256. |
| S025 | DONE | `finalize::tests::same_volume_conflict_policies_are_explicit_and_safe`; ask/overwrite/counter-rename/resume-if-same all preserve complete-file semantics. |
| S026 | DONE | `finalize::tests`: destination-local staging is copied, synced, SHA-256 verified, renamed, and failure leaves the destination/source safe. |
| S027 | DONE | `transfer::tests::pause_stops_the_stream_at_an_exact_synced_offset` and `storage::tests::graceful_pause_persists_exact_single_offset_atomically`. |
| S028 | DONE | `resume::tests`: matching object resumes only the missing range; changed ETag or size refuses before opening the partial file. |
| S029 | DONE | `resume::tests::server_without_range_support_requires_restart_without_appending`; partial bytes remain unchanged and no resume GET occurs. |
| S030 | DONE | `checkpoint::tests::fake_clock_and_byte_threshold_bound_checkpoint_writes`; 5-second/8-MiB thresholds trigger first and reset cleanly. |
| S031 | DONE | `recovery::tests::database_ahead_behind_and_orphans_repair_safely`; file-ahead truncates, DB-ahead rewinds, missing/orphan partials are preserved and reported. |
| S032 | DONE | `recovery::tests::twenty_seeded_process_kills_recover_without_corrupt_completion`; 20 real child processes are force-killed, reconciled, completed, and byte-verified. |
| S033 | DONE | `retry::tests::deterministic_backoff_has_bounded_jitter_and_budget`; seeded exponential jitter is bounded, capped, and exhausts deterministically. |
| S034 | DONE | `retry::tests::retry_after_seconds_date_malformed_and_status_mapping`; delta/date parsing and 408/425/429/5xx mapping pass. |
| S035 | DONE | `integrity::tests::sha256_correct_wrong_and_cancel_during_verify`; SHA-256 streams, matches, rejects, and cancels between chunks. |
| S036 | DONE | `integrity::tests::user_hash_algorithm_format_and_mismatch_table`; MD5/SHA-1/SHA-256/SHA-512 raw/prefixed/case formats pass, weak hashes are labeled compatibility-only. |
| S037 | DONE | `disk::tests`: preflight rejects impossible capacity; injected runtime disk-full returns an actionable pause error with the exact safe offset. |
| S038 | DONE | `segmented::tests::delayed_ranges_write_without_overlap_or_gap_and_match_final_hash`; two concurrent 206 workers write adjacent positional ranges into a preallocated file and verify the final SHA-256. |
| S039 | DONE | `segmented::tests::fixed_planner_covers_boundaries_small_objects_and_large_files`; 1–32 balanced adjacent ranges cover tiny, uneven, and 1-TiB objects without gaps. |
| S040 | DONE | `segmented::tests::restart_requests_only_persisted_missing_segment_suffixes`; reopened SQLite offsets request only missing suffixes, rebuild the exact file, and atomically checkpoint completed segments/job bytes. |
| S041 | DONE | `segmented::tests::ignored_or_invalid_content_range_falls_back_without_corruption`; ignored ranges and mismatched `Content-Range` discard positional output and restart a clean single stream. |
| S042 | DONE | `scheduler::tests::slow_worker_fixture_steals_from_largest_remaining_segment`; an idle worker bisects the largest active remainder while preserving exact unfinished coverage. |
| S043 | DONE | `scheduler::tests::minimum_split_size_prevents_tiny_segments_and_records_idle_workers`; split tails remain at least 128 bytes and satisfied/unsatisfied idle events are counted. |
| S044 | DONE | `network_metrics::tests::keep_alive_fixture_reports_one_handshake_and_one_reuse`; two real HTTP requests reuse one keep-alive connection and expose request/handshake/reuse counters. |
| S045 | DONE | `adaptive::tests::shaped_fast_server_ramps_from_one_to_bounded_thirty_two`; productive saturated intervals ramp 1→2→4→8→16→32 while respecting lower configured caps. |
| S046 | DONE | `adaptive::tests::throttle_loss_and_slow_disk_fixtures_reduce_load_and_hold_cooldown`; 429-style throttle, retry/loss, and queued disk writes halve concurrency and suppress immediate reramp. |
| S047 | DONE | `limits::tests::virtual_time_global_and_job_rates_stay_within_five_percent`; hierarchical token buckets stay within ±5% at both per-job and aggregate limits. |
| S048 | DONE | `limits::tests::global_and_per_host_caps_never_exceed_under_many_jobs`; owned permits enforce case-normalized host and aggregate connection ceilings. |
| S049 | DONE | `fairness::tests::priority_has_weighted_advantage_without_starvation`; deficit round robin produces exact 4:2:1 service while bounding low-priority wait. |
| S050 | DONE | `network_metrics::tests::forced_retry_reports_unique_wire_and_retry_bytes_separately`; overlapping retry ranges increase wire/retry totals without double-counting unique coverage. |
| S051 | TODO | — |
| S052 | TODO | — |
| S053 | TODO | — |
| S054 | TODO | — |
| S055 | TODO | — |
| S056 | TODO | — |
| S057 | TODO | — |
| S058 | TODO | — |
| S059 | TODO | — |
| S060 | TODO | — |
| S061 | TODO | — |
| S062 | TODO | — |
| S063 | TODO | — |
| S064 | TODO | — |
| S065 | TODO | — |
| S066 | TODO | — |
| S067 | TODO | — |
| S068 | TODO | — |
| S069 | TODO | — |
| S070 | TODO | — |
| S071 | TODO | — |
| S072 | TODO | — |
| S073 | TODO | — |
| S074 | TODO | — |
| S075 | TODO | — |
| S076 | TODO | — |
| S077 | TODO | — |
| S078 | TODO | — |
| S079 | TODO | — |
| S080 | TODO | — |
| S081 | TODO | — |
| S082 | TODO | — |
| S083 | TODO | — |
| S084 | TODO | — |
| S085 | TODO | — |
| S086 | TODO | — |
| S087 | TODO | — |
| S088 | TODO | — |
| S089 | TODO | — |
| S090 | TODO | — |
| S091 | TODO | — |
| S092 | TODO | — |
| S093 | TODO | — |
| S094 | TODO | — |
| S095 | TODO | — |
| S096 | TODO | — |
| S097 | TODO | — |
| S098 | TODO | — |
| S099 | TODO | — |
| S100 | TODO | — |
| S101 | TODO | — |
| S102 | TODO | — |
| S103 | TODO | — |
| S104 | TODO | — |
| S105 | TODO | — |
| S106 | TODO | — |
| S107 | TODO | — |
| S108 | TODO | — |
| S109 | TODO | — |
| S110 | TODO | — |
| S111 | TODO | — |
| S112 | TODO | — |
| S113 | TODO | — |
| S114 | TODO | — |
| S115 | TODO | — |
| S116 | TODO | — |
| S117 | TODO | — |
| S118 | TODO | — |
| S119 | TODO | — |
| S120 | TODO | — |
| S121 | TODO | — |
| S122 | TODO | — |
| S123 | TODO | — |
| S124 | TODO | — |
| S125 | TODO | — |
| S126 | TODO | — |
| S127 | TODO | — |
| S128 | TODO | — |
| S129 | TODO | — |
| S130 | TODO | — |
| S131 | TODO | — |
| S132 | TODO | — |
| S133 | TODO | — |
| S134 | TODO | — |
| S135 | TODO | — |
| S136 | TODO | — |
| S137 | TODO | — |
| S138 | TODO | — |
| S139 | TODO | — |
| S140 | TODO | — |
| S141 | TODO | — |
| S142 | TODO | — |
| S143 | TODO | — |
| S144 | TODO | — |
| S145 | TODO | — |
| S146 | TODO | — |
| S147 | TODO | — |
| S148 | TODO | — |
| S149 | TODO | — |
| S150 | TODO | — |
| S151 | TODO | — |
| S152 | TODO | — |
| S153 | TODO | — |
| S154 | TODO | — |
| S155 | TODO | — |
| S156 | TODO | — |
| S157 | TODO | — |
| S158 | TODO | — |
| S159 | TODO | — |
| S160 | TODO | — |
| S161 | TODO | — |
| S162 | TODO | — |
| S163 | TODO | — |
| S164 | TODO | — |
| S165 | TODO | — |
| S166 | TODO | — |
| S167 | TODO | — |
| S168 | TODO | — |
| S169 | TODO | — |
| S170 | TODO | — |
| S171 | TODO | — |
| S172 | TODO | — |
| S173 | TODO | — |
| S174 | TODO | — |
| S175 | TODO | — |
| S176 | TODO | — |
| S177 | TODO | — |
| S178 | TODO | — |
| S179 | TODO | — |
| S180 | TODO | — |
| S181 | TODO | — |
| S182 | TODO | — |

## Milestone evidence log

| Milestone | Command or artifact | Result |
|---|---|---|
| M0 | `node scripts/verify-markdown.mjs` | Passed: 194 unique requirement IDs and valid local links. |
| M0 | `node scripts/verify-status.mjs` | Passed: exactly one status row for each of 183 slices. |
| M0 | `powershell -File scripts/rust-quality.ps1` | Passed: format, clippy with warnings denied, and workspace tests. |
| M0 | `powershell -File scripts/typescript-quality.ps1` | Passed: format, lint, build, typecheck, and two smoke tests. |
| M0 | [GitHub Actions `Baseline CI`](https://github.com/musharaf-hossain-cs/Download-Manager/actions/runs/40199752939) | Passed on Windows: documentation, Rust, and TypeScript jobs. |
| M0 | [GitHub Actions `Security policy`](https://github.com/musharaf-hossain-cs/Download-Manager/actions/runs/40199752905) | Passed on Windows: dependency/license audit and full-history secret scan. |
| M1 | `cargo +1.99.1-x86_64-pc-windows-gnu test --workspace --locked` | Passed: 19 tests across engine, CLI, and native host; domain/state/storage/IPC cases included. |
| M1 | `cargo +1.99.1-x86_64-pc-windows-gnu clippy --workspace --all-targets -- -D warnings` | Passed; the GNU target was used because this machine has no MSVC linker. |
| M1 | `pnpm run format:check && pnpm run lint && pnpm run build && pnpm run typecheck && pnpm test` | Passed: formatting, lint, build, typecheck, and both smoke-test packages. |
| M1 | `cargo audit --deny warnings`; `cargo deny check bans licenses sources`; Node audit/license scripts | Passed: no known Rust/Node vulnerability or dependency-policy failure. |
| M2 | `cargo +1.99.1-x86_64-pc-windows-gnu test --workspace --locked` | Passed: 45 engine tests plus CLI/native-host integration tests; the child-process helper is intentionally ignored outside its 20-kill parent harness. |
| M2 | `cargo +1.99.1-x86_64-pc-windows-gnu fmt --all -- --check`; `cargo +1.99.1-x86_64-pc-windows-gnu clippy --workspace --all-targets -- -D warnings` | Passed with the installed 64-bit CodeBlocks MinGW linker selected ahead of the incompatible legacy MinGW installation. |
| M2 | `pnpm run format:check && pnpm run lint && pnpm run build && pnpm run typecheck && pnpm test` | Passed: both TypeScript packages build/typecheck and 5 tests pass across 4 files. |
| M2 | `cargo audit --deny warnings`; `cargo deny check bans licenses sources`; Node audit/license scripts | Passed: no known Rust/Node vulnerabilities; dependency sources and licenses accepted, including the HTTPS root-data `CDLA-Permissive-2.0` license. |

