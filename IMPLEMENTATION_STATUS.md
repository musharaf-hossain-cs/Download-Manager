# Implementation Status

- Last updated: 2026-10-28
- Current milestone: Milestone 8 — Queues, scheduler and system policy (next slice: S134)
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
| S051 | DONE | `proxy::tests`: local manual proxy routing and exact/suffix/wildcard/`<local>` bypass matrix pass; the full suite also repeats multi-range resume with independent Windows file handles. |
| S052 | DONE | `auth::tests::basic_and_digest_succeed_and_bad_credentials_do_not_loop`; Basic and RFC-style MD5 Digest authenticate on one retry, while rejection stops after two total requests. |
| S053 | DONE | `credentials::tests::windows_store_round_trips_without_plaintext_database_or_log_value`; the current-user Windows vault round-trips/deletes the secret while SQLite and debug output contain only a non-secret reference. |
| S054 | DONE | `network::tests::dual_stack_fixture_falls_back_after_failed_ipv6_attempt`; system-resolved addresses use bounded per-address attempts and fall back from IPv6 to IPv4. |
| S055 | DONE | `benchmark::tests::raw_metric_json_has_goodput_cpu_memory_connections_and_hash`; stable JSON records goodput, CPU, peak memory, wire/unique bytes, connection/handshake counts, and SHA-256. |
| S056 | DONE | `benchmark::tests::two_fixture_runs_produce_reproducible_comparison_report`; two named metric captures generate identical JSON-compatible comparisons and stable Markdown with basis-point goodput deltas. |
| S057 | DONE | Desktop `desktop-shell.test.ts` launch/health/close/invalid-message tests and Rust `desktop::tests` typed dispatcher tests pass. |
| S058 | DONE | `desktop::tests::health_and_persisted_jobs_are_dispatched_over_typed_ipc` plus desktop `download-list.test.ts` loading/ready/empty/error states pass. |
| S059 | DONE | Desktop `progress.test.ts` proves 20,000 synthetic events schedule one 250 ms latest-value batch and remain independent per job. |
| S060 | DONE | Desktop `virtual-list.test.ts` verifies stable sorting, immutable source order, bounded viewport rows, and the 10,000-row 250 ms budget. |
| S061 | DONE | Desktop `add-url-dialog.test.ts` plus engine `desktop::tests::url_probe_*` cover valid/invalid HTTP(S), Unicode filenames and paths, metadata preview, and inaccessible destinations; full Rust, TypeScript, and documentation gates pass. |
| S062 | DONE | Engine `desktop::tests::start_choices_persist_exactly_one_job_in_the_requested_state` and `duplicate_choices_focus_add_or_atomically_replace_one_durable_job`, plus desktop dialog choice mapping; full Rust, TypeScript, and documentation gates pass. |
| S063 | DONE | Engine `desktop::tests::start_pause_resume_and_stop_controls_are_atomic_and_journaled` plus invalid-action durability test; desktop `download-list.test.ts` verifies acknowledged controls and optimistic rollback; full gates pass. |
| S064 | DONE | Engine `desktop::tests::cancel_and_record_only_removal_preserve_the_exact_local_file` and `delete_file_requires_confirmation_for_the_exact_job_target`; desktop confirmation-binding tests; full gates pass. |
| S065 | DONE | Desktop `download-list.test.ts` mixed-state bulk resume, stop, and record-only delete-completed tests cover deduplication plus succeeded/failed/skipped reporting; full gates pass. |
| S066 | DONE | Desktop `segment-detail.test.ts` fake snapshots verify per-connection progress and speed, aggregate speed/ETA, complete zero ETA, and unknown/stalled ETA handling; full gates pass. |
| S067 | DONE | Desktop `categories.test.ts` verifies the deterministic six-category default set, per-category destinations, remembered-folder policy, and invalid root rejection; full gates pass. |
| S068 | DONE | Desktop `categories.test.ts` precedence table verifies ordered first-match extension/MIME/host rules, normalization, MIME wildcards/parameters, visible priority, and General fallback; full gates pass. |
| S069 | DONE | Desktop `history.test.ts` verifies completed-only full-text/category/date filtering, validated retention, and record-only clear operations that never target downloaded files; full gates pass. |
| S070 | DONE | Desktop `settings.test.ts` verifies multi-field search, typed persisted updates, per-section/all reset, persistence rollback, and preference-only ports that preserve jobs/files; full gates pass. |
| S071 | DONE | Desktop `appearance.test.ts` verifies light/dark/system resolution, persisted column visibility/order across reopen, new-column normalization, and persistence rollback; full TypeScript gates pass. |
| S072 | DONE | Desktop `tray.test.ts` verifies all tray commands, persisted minimize/exit/ask close modes, rollback on persistence failure, and input validation; full TypeScript gates pass. |
| S073 | DONE | Desktop `notifications.test.ts` verifies deterministic Windows completion/failure toasts, enable/support gating, invalid detail rejection, and the real-adapter smoke payload; full TypeScript gates pass. |
| S074 | DONE | Desktop `clipboard.test.ts` verifies opt-in detection, HTTP(S) validation, normalized duplicate debounce, and prompt re-entry after the debounce window; full TypeScript gates pass. |
| S075 | DONE | Desktop `drag-drop.test.ts` verifies ordered URL/text/list normalization, Unicode URLs, duplicate reporting, and positioned invalid/unsupported item reports; full TypeScript gates pass. |
| S076 | DONE | Desktop `desktop-actions.test.ts` verifies persisted/clamped floating-target state, enabled-only normalized drops, exact completed-file/folder paths, and persistence rollback; full desktop tests and typecheck pass. |
| S077 | DONE | Desktop `localization.test.ts` verifies complete externalized English/Bangla catalogs, locale fallback, missing/unknown-key rejection, and Unicode-safe 30% text-expansion fixtures; full desktop tests and typecheck pass. |
| S078 | DONE | Desktop `accessibility.test.ts` automated scan verifies named application/toolbar/grid roles, column and row semantics, non-color state cues, and Windows high-contrast tokens; deterministic keyboard walkthrough verifies roving focus, disabled-control skipping, activation, reverse navigation, and a visible focus ring; full TypeScript gates pass. |
| S079 | DONE | Native-host `partial_reads_produce_one_framed_validated_response`, oversize/invalid-frame rejection, truncated-input, and broken-pipe tests pass; targeted format, clippy, and Rust tests pass. |
| S080 | DONE | Native-host packaged-origin/current-user rejection tests verify exact allowlisting and envelope binding; `authorized_browser_origin_receives_health_response` proves the browser launch contract; full Rust/document gates pass. |
| S081 | DONE | Windows PowerShell `test-native-host-dev.ps1` verifies isolated HKCU-shaped Chrome/Edge register, framed health connect, exact-origin manifest, and unregister cleanup without elevation or real-registry changes. |
| S082 | DONE | Extension manifest tests verify a clean-installable MV3 shell with no site access and packaged-code-only CSP; full TypeScript quality gates pass. |
| S083 | DONE | Extension `connection-health.test.ts` verifies correlated connected state, native-port disconnection, host/protocol version mismatch, and distinct non-color badge states; full TypeScript gates pass. |
| S084 | DONE | Extension `link-handoff.test.ts` verifies one eligible native request, exactly one accepted job ID, acknowledgment-before-browser-cancel ordering, duplicate suppression, and no cancel on rejection/invalid response; full TypeScript gates pass. |
| S085 | DONE | Extension `link-handoff.test.ts` verifies once-only fallback with the original URL for offline, rejected, malformed, and timed-out handoffs; accepted handoffs never fallback and failed handoffs never cancel; full TypeScript gates pass. |
| S086 | DONE | Extension `capture-rules.test.ts` decision table verifies normalized extension and exact/wildcard MIME matches, minimum-size precedence, unknown-size eligibility, invalid size metadata, and explicit decision reasons; focused extension tests and typecheck pass. |
| S087 | DONE | Extension `site-rules.test.ts` verifies global allow/deny fallback, exact and wildcard site overrides, specificity precedence, deny-on-tie behavior, apex separation, and malformed input rejection; focused extension tests and typecheck pass. |
| S088 | DONE | Extension `modifier-keys.test.ts` verifies documented Ctrl-force/Alt-bypass defaults, configurable alternatives, invalid binding rejection, and deterministic bypass-on-conflict behavior; focused extension tests and typecheck pass. |
| S089 | DONE | Extension `manifest.test.ts` verifies the least-required context-menu permission, link-only command registration, normalized direct and relative HTTP(S) dispatch, and rejection of unrelated/unsafe targets; focused extension tests and typecheck pass. |
| S090 | DONE | Extension `all-links.test.ts` verifies direct/relative HTTP(S) normalization, fragment-insensitive stable deduplication, the 128-item native-batch cap, exact scan bounds, and 80 cooperative yields while processing a 20,000-candidate large-DOM fixture; full TypeScript and documentation gates pass. |
| S091 | DONE | Extension `selected-links.test.ts` verifies selection-only collection, normalized duplicate removal across overlapping ranges, and empty-selection behavior; full extension tests and typecheck pass. |
| S092 | DONE | Desktop `batch-preview-dialog.test.ts` verifies URL/filename/type/size/source preview, search and MIME filtering, visible selection, editing, queue choice, and that only checked normalized HTTP(S) URLs reach the job port; full TypeScript and documentation gates pass. |
| S093 | DONE | Extension `browser-profile.test.ts` verifies browser/profile composite identity, two-profile metadata isolation, cross-browser separation, and invalid identity rejection; full TypeScript gates pass. |
| S094 | DONE | Extension `browser-profile.test.ts` verifies incognito default-off rejection, explicit session opt-in, end-of-session clearing, and zero private persistence calls; manifest tests verify split incognito execution; full TypeScript gates pass. |
| S095 | DONE | Desktop `browser-integration-diagnostics.test.ts` verifies healthy and broken extension/version, Chrome/Edge registration, manifest, executable, allowlist, connection, protocol, and native-host fixtures plus deduplicated repair/recheck and failure preservation; extension `connection-health.test.ts` verifies exact mismatch evidence opens the repair flow; full verification gates pass. |
| S096 | DONE | Firefox Manifest V3 shares the Chromium identity/permissions and compiled TypeScript source; `web-ext` 10.8.0 lint and ZIP build pass, with focused manifest tests and full TypeScript/document gates green. |
| S097 | DONE | Firefox `allowed_extensions` manifest/HKCU registration smoke passes; native-host unit/process tests bind the exact manifest path plus add-on ID to the running executable, packaged allowlist, request envelope, and current user while rejecting wrong IDs/manifests. |
| S098 | DONE | Extension `firefox-link-interception.test.ts` runs stable/ESR fixtures through the shared handoff contract, proving one request/job, accepted-ACK-before-cancel ordering, duplicate suppression, optional-field compatibility, untouched fallback transfers, and typed reporting when browser cancellation fails; full TypeScript/document gates pass. |
| S099 | DONE | Extension `firefox-browser-behavior.test.ts` runs stable/ESR shared contracts for filtered installed interception, unknown-size normalization, site/type rules, force/bypass keys, and lifecycle-safe single/all/selected context menus with bounded normalized batches; full TypeScript/document gates pass. |
| S100 | DONE | `pnpm test:browser-regression` runs one Chrome/Edge/Firefox stable+ESR adapter-contract matrix for acknowledged handoff, fallback preservation, capture rules/keys, and single/all/selected menus, then writes the stable `test-results/browser-regression.json` artifact; full TypeScript/document gates pass. |
| S101 | DONE | Extension `media-candidates.test.ts` verifies normalized progressive/HLS/DASH classification, stable cross-source/player deduplication, SPA invalidation, immutable bounded snapshots, and malformed/false-positive rejection; full extension and documentation gates pass. |
| S102 | DONE | Extension `media-player-overlay.test.ts` verifies a closed Shadow-DOM download button, fixed player-corner placement, click/cleanup behavior, clipped-player viewport gutter, and unchanged host player layout/classes/controls; full extension and documentation gates pass. |
| S103 | DONE | Extension `media-overlay-lifecycle.test.ts` verifies coalesced dynamic insertion/removal, fragment-stable and route-changing SPA identities, theater/zoom/player-resize repositioning, fullscreen portal changes, observer cleanup, and multiple eligible players; full TypeScript and documentation gates pass. |
| S104 | DONE | Extension `media-player-overlay.test.ts` verifies isolated minimize/drag controls and state reporting; `media-overlay-interaction.test.ts` verifies persisted/clamped per-site state, global/exact-site enable exceptions, malformed-state normalization, and persistence rollback; full TypeScript and documentation gates pass. |
| S105 | DONE | Extension `progressive-media-handoff.test.ts` verifies acknowledged MP4/WebM/audio handoff with the required normalized referrer, exact fixture bytes/SHA-256, duplicate suppression, bounded timeout/retry, and invalid/offline/native-rejection handling; full TypeScript and documentation gates pass. |
| S106 | DONE | Engine `media::tests` verify HLS master variants plus external/muxed audio and subtitle URI resolution, bitrate/resolution/frame-rate/codec/language metadata, and rejection of media playlists, malformed tags, missing stream URIs, and non-HTTP targets; focused and established Rust/document gates pass. |
| S107 | DONE | Engine `media::tests` parse ordered TS and fMP4 media playlists with resolved init maps, explicit/implicit byte ranges, media sequences, discontinuities, end markers, and malformed-input rejection; focused and established Rust/document gates pass. |
| S108 | DONE | Engine `hls::tests` verify init/media objects are fetched and written in playlist order, retryable failures recover within budget, missing fragments stop safely, byte ranges require exact partial responses, and final bytes/SHA-256 match; full Rust/document gates pass. |
| S109 | DONE | Engine `hls::tests::restart_skips_durable_units_and_truncates_uncheckpointed_tail` reopens SQLite after a simulated crash, validates the durable identity/content-hashed prefix, truncates file-ahead bytes, requests only missing HLS units, and produces the exact final bytes/hash; full verification gates pass. |
| S110 | DONE | `dash::tests` parse static multi-period MPDs with inherited/nested `BaseURL`, effective adaptation/representation metadata, namespaced DRM markers, and bounded malformed/dynamic-input rejection. |
| S111 | DONE | Engine `dash::tests` verify bounded `SegmentTemplate` substitution/inheritance, duration and timeline expansion (including `r=-1` boundaries), ordered `SegmentList` URLs/ranges, and malformed/unbounded-input rejection; all fast verification gates pass. |
| S112 | DONE | Engine `dash_download::tests` verify concurrent audio/video transfer with isolated durable jobs, exact track bytes/SHA-256 hashes, checkpointed counters, crash-tail truncation, and restart requests for missing resources only; all fast verification gates pass. |
| S113 | DONE | Engine `media_worker::tests` verify shell-free literal metacharacter arguments, canonical local-file-only inputs, crash/timeout cleanup with source preservation, and bounded input/output/stderr resources; all fast verification gates pass. |
| S114 | DONE | Engine `media_mux::tests` verify deterministic MP4/MKV/WebM codec compatibility, bounded local-only ffprobe duration/start/end-sync/codec assertions, and invalid-output cleanup with both source tracks preserved; all fast verification gates pass. |
| S115 | DONE | Engine `audio_output::tests` verify deterministic direct-format preservation and copy-only segmented container plans, exact direct/HLS/DASH bytes and SHA-256 hashes, ordered HLS init/media output, and DASH audio-only requests with no video fetch; all fast verification gates pass. |
| S116 | DONE | Engine `subtitle::tests` verify selected WebVTT/TTML downloads with exact source hashes, millisecond-exact SRT timing, multilingual Unicode, plain-text markup handling, bounded complexity, malformed/ambiguous timing rejection, and cleanup with no partial output; all fast verification gates pass. |
| S117 | DONE | Engine `media_selection::tests` verify bounded HLS/DASH menus with quality/language/codec/container and known-or-estimated size metadata, exact audio/video/subtitle track identity mapping, HLS group compatibility, audio-only selection, and DASH period isolation; all fast verification gates pass. |
| S118 | DONE | Engine `media_refresh::tests` verify explicit original browser/profile/session scope and signature-only HLS/DASH identity refresh; refreshed HLS plus DASH plan/checkpoint-preflight tests prove only missing units resume while locator, semantic-query, topology, and stale-checkpoint mismatches preserve partial files and checkpoints; all fast verification gates pass. |
| S119 | DONE | Engine request-context and storage tests verify sanitized media job URLs, opaque vault references, SQLite/WAL/log/export secret scans, schema-v3 persistence, pre-expiry retention, retry-safe vault cleanup, and expired-row removal; all fast verification gates pass. |
| S120 | DONE | Engine HLS parser and DASH selection tests verify fail-closed Widevine, PlayReady, FairPlay, common-encryption, generic-key, and unknown protection classification without exposing key, KID, PSSH, or license material; protected fixtures cannot reach track selection or download; all fast verification gates pass. |
| S121 | DONE | Engine `hls::tests` verify clear live rolling-window deduplication, rollover gap reporting, complete-segment duration stops, cooperative cancellation before the next request, end-list completion, exact bytes/duration/SHA-256 accounting, and partial preservation on transfer failure; all fast verification gates pass. |
| S122 | DONE | Engine schema v4 persists validated named queues with atomic, exact-permutation ordering; `storage::tests::named_queue_order_is_atomic_and_stable_across_restart` verifies creation order, reorder rollback, timestamps, positions, and restart stability; engine format/clippy pass, with 135 tests passed and one recovery helper ignored. |
| S123 | DONE | Desktop `queues.test.ts` verifies ordered cross-queue move/copy, exact-permutation reorder, mixed-state manual start/stop eligibility, deduplicated missing-job reporting, authoritative snapshots, and rejected-mutation rollback; full desktop tests, typecheck, and documentation gates pass. |
| S124 | DONE | Engine `limits::tests::per_queue_caps_never_exceed_under_mixed_jobs` verifies independent named-queue permits never exceed configured concurrent-download caps, released slots are reusable, and active counts return to zero; invalid names, zero caps, and unknown queues are rejected; focused engine tests and clippy pass. |
| S125 | DONE | Engine `queue_schedule::tests` use a fake-clock next-run table to verify one-time and normalized weekly recurrence across UTC, Dhaka, New York, and Berlin IANA zones; invalid zones/empty weekdays are rejected and DST gaps/folds remain explicit policy errors for S126; focused engine tests and clippy pass. |
| S126 | DONE | Engine `queue_schedule::tests` verify configurable missed-run skip/run-on-wake decisions, coalesced long-offline occurrences, durable-watermark restart deduplication, nonexistent DST gaps, and once-only earlier-instant behavior for folds; full Rust quality gates pass with 142 tests and one recovery helper ignored. |
| S127 | DONE | Engine `queue_stop::tests` use fake time and cumulative metrics to verify stop-at-time plus completed-file, transferred-byte, and failure thresholds below/at/above each boundary, stable simultaneous-condition precedence, disabled conditions, zero rejection, and overflow-safe maxima; full Rust quality gates pass with 147 tests and one recovery helper ignored. |
| S128 | DONE | Desktop `scheduler-health.test.ts` verifies distinct disabled, unavailable, idle-healthy, and authoritative next-run snapshots, failed health-request warning fallback, and malformed snapshot rejection; all 99 desktop tests, typecheck, focused lint, and formatting checks pass. |
| S129 | DONE | Engine `power_action::tests` use a mock adapter and fake timestamps to verify visible remaining time, exact-deadline once-only execution, terminal adapter failures, zero-duration rejection, and that cancellation prevents every OS action; the Windows adapter maps sleep/hibernate/sign-out/shutdown behind the countdown boundary; focused engine tests and clippy pass. |
| S130 | DONE | Engine `scheduled_wake::tests` verify supported adapters install/replace and cancel queue wake requests, unsupported platforms return a stable capability outcome without OS calls, disabling is safe, and invalid/elapsed requests fail before adapter use; focused engine tests and clippy pass. |
| S131 | DONE | Engine `transfer_quota::tests` use virtual time to verify partial grants, rolling usage expiry, anchor-aligned configured resets, reported resume times, automatic availability at reset, and fail-closed invalid configuration or clock movement; focused engine tests and clippy pass. |
| S132 | DONE | Engine `bandwidth_schedule::tests` verify IANA-zone weekly global/exact-queue limits, lowest-cap overlap, unioned quiet state, start-inclusive/end-exclusive boundaries, and start-weekday-owned overnight windows; malformed/no-op rules fail closed, and focused tests plus strict clippy pass. |
| S133 | DONE | Engine `system_policy::tests` use a mock Windows-state adapter to verify metered ask/pause/allow, roaming default-pause, battery speed/connection reduction and known-large-job pause, fail-closed unavailable state, and owner-aware recovery that resumes only system-policy pauses; focused tests and strict clippy pass. |
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
| M3 | `cargo +1.99.1-x86_64-pc-windows-gnu test --workspace --locked` | Passed: 65 engine tests plus CLI/native-host integration tests; the child-process recovery helper remains intentionally ignored outside its parent harness. |
| M3 | `cargo +1.99.1-x86_64-pc-windows-gnu fmt --all -- --check`; `cargo +1.99.1-x86_64-pc-windows-gnu clippy --workspace --all-targets -- -D warnings` | Passed; segmented I/O, schedulers, limits, proxy/auth, credential storage, networking, and benchmark modules are clean. |
| M3 | `pnpm run format:check`; `pnpm run lint`; `pnpm run build`; `pnpm run typecheck`; `pnpm test` | Passed: both TypeScript packages build/typecheck and 5 tests pass across 4 files. |
| M3 | `cargo audit --deny warnings`; `cargo deny check bans licenses sources`; Node audit/license scripts; Gitleaks full-history scan | Passed: no known vulnerabilities or leaks; dependency sources/licenses accepted (duplicate-version policy warnings only). |
| M4 | `powershell -File scripts/verify.ps1` with the pinned Rust GNU toolchain selected | Passed: documentation/status checks, Rust format/clippy and 75 engine tests plus CLI/native-host tests, and TypeScript format/lint/build/typecheck with 76 desktop and 3 browser tests. |
| M5 | `powershell -File scripts/verify.ps1` with the pinned Rust GNU toolchain selected | Passed: documentation/status checks; Rust format/clippy plus 75 engine and 8 CLI/native-host tests (one helper ignored); TypeScript format/lint/build/typecheck plus 86 desktop and 51 browser-extension tests. |

