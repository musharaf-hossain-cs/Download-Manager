# Quota-Safe Progressive Implementation Plan

| বিষয় | মান |
|---|---|
| Status | Execution baseline |
| Version | 1.1 |
| Date | 2026-10-25 |
| Source requirements | [`REQUIREMENTS.md`](./REQUIREMENTS.md) |
| Planning constraint | Each atomic slice must fit comfortably inside one Codex 5-hour **usage quota window**, even when that quota is consumed in roughly 1–2 hours of intensive work |

## 1. উদ্দেশ্য

পুরো application একবারে implement করা হবে না। কাজটি 12টি milestone এবং 183টি ছোট atomic slice-এ ভাগ করা হয়েছে। একটি Codex session সাধারণত **একটি Q2 slice** অথবা সর্বোচ্চ **দুটি adjacent Q1 slice** নেবে। এতে context, tool calls, compilation, tests, debugging এবং commit—সব usage limit-এর ভেতরে রাখার safety margin থাকে।

এখানে “quota-safe” wall-clock guarantee নয়। কোনো slice unexpectedly জটিল হলে scope/test কমিয়ে pass দেখানো যাবে না; remaining root cause নতুন continuation slice হবে।

---

## 2. Slice size contract

### Quota weights

| Weight | Intended size | Session rule |
|---|---|---|
| Q1 | One small isolated change, usually ≤3 files and ≤250 net production LOC | At most two adjacent Q1 slices if the first is fully complete |
| Q2 | One focused behavior across one component, usually ≤6 files and ≤500 net production LOC | Exactly one slice in a session |

No planned item is larger than Q2. Generated files/migrations/fixtures are not counted as production LOC, but their review/test cost still matters.

### Every slice includes

1. Inspect current code and relevant requirements.
2. Implement only the named behavior.
3. Add/update focused automated tests.
4. Run the focused tests plus established fast regression checks.
5. Update `IMPLEMENTATION_STATUS.md` with evidence.
6. Review the diff and commit one cohesive change.

### Hard scope limits

- One subsystem boundary per slice: engine, UI, extension, media, packaging—never several at once.
- One new public contract per slice.
- No unrelated refactor.
- No “implement feature and all edge cases” slice; edge cases are named separately.
- At ~70% of the available usage window, stop expanding scope and finish tests/docs/commit.
- If tests do not pass, leave the slice `IN PROGRESS`; do not start the next slice.

---

## 3. Provisional architecture

Slice S004 validates and records the final decision. The plan assumes:

- Stable Rust + Tokio for engine, native host and CLI.
- SQLite for durable state; Windows Credential Manager/DPAPI-backed secret storage.
- Tauri 2 + TypeScript for the Windows desktop UI.
- TypeScript WebExtensions: Chromium Manifest V3 and Firefox build.
- Maintained FFmpeg/ffprobe integration in a restricted worker process.
- Rust tests, Vitest and a programmable local HTTP/media fixture server.

If the architecture spike finds a blocking issue, update the ADR and this plan before feature work.

---

## 4. Per-session workflow

1. Read `IMPLEMENTATION_STATUS.md`; choose the first unblocked `TODO` slice.
2. Confirm all dependencies are `DONE`.
3. Set only that slice to `IN PROGRESS`.
4. Implement, test and document it.
5. Run the repository fast checks applicable at that point.
6. Record commands and results, mark `DONE`, review `git diff`, commit.
7. End the session. Do not start a non-adjacent slice with leftover quota.

Suggested commit format:

```text
type(scope): complete SNNN short description
```

---

## 5. Atomic slice catalog

### Milestone 0 — Public repository baseline

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S000 | Q1 | Requirements, quota-safe plan, README and repository hygiene files | Markdown/ID checks; initial public push | — |
| S001 | Q1 | Create workspace folders and pin Rust/Node/package-manager versions | Clean checkout tool-version and empty-workspace build | S000 |
| S002 | Q1 | Add minimal Rust engine/CLI/native-host crates with `--version` | `cargo test --workspace`; version snapshots | S001 |
| S003 | Q1 | Add minimal desktop and browser TypeScript packages | Install, typecheck and one smoke test | S001 |
| S004 | Q2 | ADR: architecture, process boundaries, dependency policy | ADR review checklist; sample build proves chosen stack | S002–S003 |
| S005 | Q1 | Create `IMPLEMENTATION_STATUS.md` and traceability template | Script verifies every known slice has one status row | S004 |
| S006 | Q1 | Rust format/lint/test local command | Deliberate failure is caught, then reverted | S002 |
| S007 | Q1 | TypeScript format/lint/typecheck/test local command | Deliberate failure is caught, then reverted | S003 |
| S008 | Q2 | Windows CI for build and fast tests | One green GitHub Actions run from clean checkout | S006–S007 |
| S009 | Q1 | Dependency/license audit and secret scan CI | Seeded fake failure is detected, then reverted | S008 |

### Milestone 1 — Domain and durable state

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S010 | Q1 | Core IDs, job metadata and remote-identity types | Serialization and invalid-value tests | S004 |
| S011 | Q1 | Canonical download state enum and event types | Stable serialization snapshots | S010 |
| S012 | Q2 | Legal state-transition reducer | Exhaustive legal/illegal transition table tests | S011 |
| S013 | Q1 | SQLite connection/configuration and schema-version table | Create/reopen temporary DB | S010 |
| S014 | Q2 | Jobs and remote-identity migration/repository | CRUD + transaction rollback tests | S013 |
| S015 | Q2 | Segments and event-log migration/repository | Range constraints and ordered-event tests | S014 |
| S016 | Q1 | Migration backup and failure rollback | Broken migration fixture leaves original DB usable | S015 |
| S017 | Q1 | Retention rules for completed history/events | Active/partial jobs are never age-deleted | S015 |
| S018 | Q1 | Versioned UI↔engine IPC message schema | Unknown-version and malformed-message rejection | S011 |
| S019 | Q1 | Versioned extension↔host schema with limits/request IDs | Oversize, duplicate ID and invalid URL tests | S018 |

### Milestone 2 — Correct single-stream engine

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S020 | Q1 | Programmable local HTTP fixture server | Status/header/body/delay fixtures self-test | S008 |
| S021 | Q2 | HTTP(S) URL probe with HEAD and small-GET fallback | HEAD denied, redirect and unknown-length fixtures | S010, S020 |
| S022 | Q1 | RFC-aware filename selection | Unicode/content-disposition/URL fallback table | S021 |
| S023 | Q1 | Windows-safe path/filename sanitizer | Traversal, ADS, device-name and control-char corpus | S022 |
| S024 | Q2 | Single stream to a temp file with progress events | Known/unknown length and hash fixtures | S014, S021 |
| S025 | Q1 | Same-volume atomic final rename and conflict policies | Ask/overwrite/rename/resume-if-same tests | S024 |
| S026 | Q1 | Cross-volume copy-verify-rename abstraction | Simulated cross-volume success/failure tests | S025 |
| S027 | Q1 | Pause command and durable single offset | Pause stops network and persists exact safe offset | S024 |
| S028 | Q2 | Range resume with remote-identity validation | Same-object resumes; changed ETag/size refuses | S027 |
| S029 | Q1 | Server-no-range restart-required behavior | No duplicated/appended output | S028 |
| S030 | Q1 | Periodic checkpoint policy | Fake clock/byte threshold tests, bounded writes | S028 |
| S031 | Q2 | Startup DB/temp-file reconciliation | DB ahead/behind/orphan fixtures repair safely | S030 |
| S032 | Q2 | Random process-kill recovery harness | 20 seeded kills; zero corrupt completed output | S031 |
| S033 | Q1 | Retry backoff, jitter and budget | Deterministic fake-clock tests | S024 |
| S034 | Q1 | `Retry-After` and 408/425/429/5xx mapping | Seconds/date/malformed and exhaustion cases | S033 |
| S035 | Q1 | Streaming SHA-256 verification | Correct/wrong/cancel-during-verify tests | S024 |
| S036 | Q1 | User-provided MD5/SHA-1/SHA-256/SHA-512 compare | Algorithm/format/mismatch table | S035 |
| S037 | Q1 | Disk-space precheck and runtime disk-full handling | Safe paused state and actionable error | S024 |

### Milestone 3 — Segmented high-speed engine

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S038 | Q2 | Two fixed non-overlapping HTTP ranges with positional writes | Delayed ranges, final hash and no overlap/gap | S032, S035 |
| S039 | Q1 | Configurable fixed 1–32 range planner | Boundary/very-small/large-file property tests | S038 |
| S040 | Q2 | Persist and resume multi-segment offsets | Kill/restart only fetches missing ranges | S039 |
| S041 | Q1 | Invalid/ignored `Content-Range` fallback | Safe single-stream fallback fixture | S038 |
| S042 | Q2 | Largest-remaining-segment dynamic split | Slow-worker fixture proves work stealing | S040 |
| S043 | Q1 | Minimum split size and idle-worker metrics | No tiny-segment explosion; idle assertion | S042 |
| S044 | Q1 | Connection-pool/handshake metrics | Keep-alive fixture shows reuse | S038 |
| S045 | Q2 | Auto-concurrency ramp-up controller | Shaped fast server ramps within 1–32 | S043–S044 |
| S046 | Q2 | Auto-concurrency backoff on retry/throttle/disk pressure | 429/loss/slow-disk fixtures reduce load | S045 |
| S047 | Q1 | Global/per-job token-bucket speed limit | Virtual-time rate within ±5% | S039 |
| S048 | Q1 | Global and per-host active connection caps | Cap never exceeded under many jobs | S045 |
| S049 | Q2 | High/normal/low weighted fairness | Priority advantage without starvation | S047–S048 |
| S050 | Q1 | Unique bytes vs wire/retry byte accounting | Forced retry reports both correctly | S034, S039 |
| S051 | Q2 | System/manual HTTP(S) proxy and no-proxy rules | Local proxy and wildcard matrix | S034 |
| S052 | Q1 | Basic/Digest authentication challenge loop | Success/bad credentials/no infinite loop | S034 |
| S053 | Q2 | Windows encrypted credential store adapter | Round trip; no plaintext DB/log value | S052 |
| S054 | Q1 | IPv4/IPv6 connection fallback | Dual-stack fixture and timeout fallback | S021 |
| S055 | Q1 | Raw engine benchmark metric recorder | JSON has goodput/CPU/memory/connections/hash | S042 |
| S056 | Q1 | Reproducible benchmark report generator | Two fixture runs produce comparison report | S055 |

### Milestone 4 — Desktop application

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S057 | Q2 | Desktop shell starts engine and performs typed health IPC | Launch/close/invalid-message smoke tests | S018, S024 |
| S058 | Q1 | Basic download list from persisted jobs | Loading/empty/error UI tests | S057 |
| S059 | Q1 | Coalesced 2–5 Hz progress updates | Synthetic high-frequency events stay bounded | S058 |
| S060 | Q1 | Virtualized/sortable 10,000-row list | Render-time budget and sort tests | S058 |
| S061 | Q2 | Add URL dialog with probe and validation | Valid/invalid/Unicode/inaccessible-path tests | S021–S023, S057 |
| S062 | Q1 | Start-now/start-later and duplicate URL choice | Exactly one durable job per choice | S061 |
| S063 | Q2 | Start/pause/resume/stop job controls | Engine rejection rolls UI state back | S059 |
| S064 | Q1 | Cancel/remove-record/delete-file separation | Exact-target confirmation tests | S063 |
| S065 | Q1 | Bulk resume/stop/delete-completed | Mixed-state selection tests | S063–S064 |
| S066 | Q2 | Segment detail and speed/ETA view | Fake event snapshots and zero/unknown ETA cases | S059 |
| S067 | Q1 | Default categories and destinations | Deterministic defaults test | S061 |
| S068 | Q2 | Ordered extension/MIME/host category rules | Precedence table and no-match fallback | S067 |
| S069 | Q1 | History search/filter/retention UI | Clearing history does not delete files | S058 |
| S070 | Q1 | Searchable settings and safe section reset | Reset preserves jobs/files | S057 |
| S071 | Q1 | Light/dark/system theme and persisted columns | Theme/layout reopen snapshots | S058, S070 |
| S072 | Q1 | System tray and configurable close behavior | Tray command/close-mode tests | S063, S070 |
| S073 | Q1 | Windows completion/failure notifications | Mock + real smoke notification | S063 |
| S074 | Q1 | Optional clipboard URL detection and debounce | Duplicate/invalid clipboard tests | S061 |
| S075 | Q1 | URL/text/list drag-and-drop | Normalization and invalid-item report | S061 |
| S076 | Q1 | Floating drop target and open file/folder actions | Toggle/persistence/path tests | S075 |
| S077 | Q1 | Externalized strings + English/Bangla skeleton | Missing-key and text-expansion checks | S057 |
| S078 | Q2 | Keyboard, screen-reader and high-contrast pass | Automated scan + keyboard walkthrough | S058–S071, S077 |

### Milestone 5 — Chromium browser integration

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S079 | Q2 | Native-messaging framing and schema validation | Partial/oversize/invalid frames and broken pipe | S019 |
| S080 | Q1 | Extension-ID allowlist and current-user IPC protection | Wrong origin/user abstraction rejected | S079 |
| S081 | Q1 | Development native-host registration/cleanup script | Register/connect/unregister smoke test | S080 |
| S082 | Q1 | Chromium MV3 extension shell with least permissions | Manifest validation and clean install | S003 |
| S083 | Q1 | Native connection health badge and error state | Connected/disconnected/version mismatch tests | S079, S082 |
| S084 | Q2 | Eligible link-click handoff with accept-before-cancel | Native success creates exactly one job | S061, S083 |
| S085 | Q1 | Failed handoff browser fallback | App offline/reject never loses link | S084 |
| S086 | Q1 | MIME/extension/min-size capture rules | Decision table with reason | S084 |
| S087 | Q1 | Global/per-site allow/deny rules | Wildcard and precedence tests | S086 |
| S088 | Q1 | Force and bypass modifier keys | Configured alternatives and conflicts | S084 |
| S089 | Q1 | Single-link context menu | Relative/direct link fixtures | S084 |
| S090 | Q2 | All-links collection, normalization and bounded dedup | Large DOM fixture remains responsive | S089 |
| S091 | Q1 | Selected-links collection | Selection-only and duplicate tests | S090 |
| S092 | Q2 | Native batch preview/filter/edit/queue dialog | Only checked normalized URLs become jobs | S090–S091 |
| S093 | Q1 | Browser/profile identity boundary | Two-profile metadata never mixes | S083 |
| S094 | Q1 | Incognito default-off and ephemeral enabled behavior | No private metadata persists | S093 |
| S095 | Q2 | Version mismatch/self-check/repair screen | Broken fixtures identify exact repair | S081, S083 |

### Milestone 6 — Firefox and browser regression

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S096 | Q1 | Firefox manifest/build from shared extension source | `web-ext` lint/build succeeds | S082 |
| S097 | Q1 | Firefox native-host manifest/origin adapter | Valid origin connects; wrong one rejected | S080, S096 |
| S098 | Q2 | Firefox link interception and fallback | Stable + ESR smoke fixtures | S084–S085, S097 |
| S099 | Q2 | Firefox rules, keys and context/batch menus | Shared behavior contract passes | S086–S092, S098 |
| S100 | Q2 | Cross-browser fixture runner and result artifact | Chrome/Edge/Firefox one-command matrix | S095, S099 |

### Milestone 7 — Media detection and download

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S101 | Q2 | Deduplicated progressive/HLS/DASH candidate model | SPA/multiple-player/false-positive fixtures | S084 |
| S102 | Q1 | Basic Shadow-DOM player overlay | Does not alter host layout/controls | S101 |
| S103 | Q2 | Overlay for SPA, zoom, theater/fullscreen and removal | DOM/visual fixture suite | S102 |
| S104 | Q1 | Overlay minimize/drag and per-site disable | Persistence and exception tests | S103 |
| S105 | Q2 | Progressive MP4/WebM/audio variant handoff | Referer-required fixtures and hashes | S084, S104 |
| S106 | Q1 | HLS master playlist parser | Variant/audio/subtitle URI corpus | S101 |
| S107 | Q2 | HLS media playlist parser | TS/fMP4/init/byte-range/discontinuity corpus | S106 |
| S108 | Q2 | HLS VOD ordered fragment downloader | Missing/retry/order/hash fixtures | S107, S042 |
| S109 | Q1 | HLS checkpoint and resume | Kill/restart skips durable fragments | S108 |
| S110 | Q2 | Static DASH adaptation/representation parser | Multi-period/BaseURL/DRM-marker corpus | S101 |
| S111 | Q2 | DASH segment template/timeline/list expansion | Boundary and malformed timeline tests | S110 |
| S112 | Q2 | DASH separate track downloader | Audio/video track hashes and resume | S111, S042 |
| S113 | Q2 | Restricted FFmpeg worker and safe argument builder | Injection/crash/timeout/resource tests | S004 |
| S114 | Q2 | MP4/MKV/WebM A/V mux compatibility rules | ffprobe duration/sync/codec assertions | S108, S112–S113 |
| S115 | Q1 | Audio-only output path | HLS/DASH/direct fixtures | S105, S114 |
| S116 | Q2 | WebVTT/TTML download and safe SRT conversion | Timing/Unicode/malformed tests | S106, S110, S113 |
| S117 | Q2 | Quality/language/codec/size variant menu | Choice maps exactly to selected tracks | S104–S116 |
| S118 | Q2 | Expired signed-URL user refresh flow | Same object resumes; mismatch refuses | S093, S109, S112 |
| S119 | Q1 | Request-context secret redaction and expiry cleanup | DB/log/export secret scan | S118 |
| S120 | Q2 | DRM/EME/protected classification and hard block | Protected fixtures never reach key/decrypt path | S106–S111 |
| S121 | Q2 | Clear live HLS recording window and stop condition | Rollover/gap/cancel/duration report | S108, S114 |

### Milestone 8 — Queues, scheduler and system policy

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S122 | Q1 | Named queue schema and stable ordering | Restart/reorder tests | S015 |
| S123 | Q2 | Queue UI: move/copy/reorder and manual start/stop | Mixed-job state tests | S065, S122 |
| S124 | Q1 | Per-queue concurrent-download limit | Engine cap tests | S048, S123 |
| S125 | Q2 | One-time/weekly recurrence with timezone | Fake-clock next-run table | S122 |
| S126 | Q1 | DST and missed-run skip/run-on-wake rules | Gap/fold/offline/restart cases | S125 |
| S127 | Q1 | Stop by time/count/bytes/failure count | Fake metrics/clock cases | S125 |
| S128 | Q1 | Scheduler next-run/health UI | Disabled/unavailable/next-run snapshots | S125–S127 |
| S129 | Q2 | Power-action adapter with confirmation countdown | Mock adapter; cancel prevents action | S127 |
| S130 | Q1 | Optional scheduled wake adapter | Supported/unsupported OS paths | S125 |
| S131 | Q2 | Rolling/configured transfer quota | Virtual clock reset and auto-resume | S047 |
| S132 | Q1 | Scheduled bandwidth and quiet hours | Boundary/overnight window tests | S047, S125 |
| S133 | Q2 | Metered/roaming/battery policy adapter | Mock Windows state, safe pause/resume | S063 |
| S134 | Q1 | Network/VPN route-change reconnect | Connection reset resumes safely | S028, S054 |

### Milestone 9 — Grabber, CLI and post-processing

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S135 | Q2 | Bounded same-host HTML link crawler | Cyclic/relative/base URL fixture | S024 |
| S136 | Q1 | CSS asset discovery and canonical dedup | CSS imports/URLs/duplicates | S135 |
| S137 | Q1 | Depth/page/byte/duration/concurrency caps | Every cap stops deterministically | S135 |
| S138 | Q1 | Include/exclude/type presets | Ordered filter table | S135 |
| S139 | Q2 | Robots/rate policy and form/logout blocking | robots/login/logout/delete fixtures | S135 |
| S140 | Q2 | Safe local mapping and HTML/CSS offline rewrite | Links stay inside destination | S136, S138 |
| S141 | Q2 | Conditional changed-only synchronization | unchanged/changed/deleted/interrupted fixtures | S125, S140 |
| S142 | Q1 | CLI add/list/status and JSON output | Golden output and exit codes | S018, S061 |
| S143 | Q1 | CLI pause/resume/cancel | State/error contract tests | S063, S142 |
| S144 | Q1 | CLI queue and quiet/wait behavior | Queue/timeout/cancel tests | S123, S142 |
| S145 | Q2 | Windows Defender/registered scanner adapter | Mock + harmless local workflow | S035 |
| S146 | Q2 | Safe external scanner/post-hook execution | Metacharacter/path/timeout/exit tests | S145 |

### Milestone 10 — Diagnostics, packaging and updates

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S147 | Q1 | Bounded structured logs and correlation IDs | Rotation/size/concurrency tests | S018 |
| S148 | Q2 | Central secret-redaction pipeline | Auth/cookie/token/signed-URL corpus | S053, S119, S147 |
| S149 | Q2 | User-previewed diagnostics and health bundle | Deterministic bundle; secret scan clean | S095, S148 |
| S150 | Q1 | Settings/categories/queues/history export format | Versioned non-secret archive test | S068, S123, S148 |
| S151 | Q2 | Transactional import with preview/merge/replace | Round-trip/old/corrupt/tampered fixtures | S150 |
| S152 | Q2 | Per-user Windows installer package | Clean VM install and launch smoke | S081, S097 |
| S153 | Q2 | Installer native-host registration/onboarding | Three-browser connection smoke | S152 |
| S154 | Q1 | Repair mode for missing/corrupt registrations | Seeded failures repaired | S153 |
| S155 | Q2 | Uninstall choices and cleanup | Registrations removed; downloads preserved | S153 |
| S156 | Q2 | Signed update metadata/hash verification with test keys | Valid/invalid/tampered tests | S152 |
| S157 | Q2 | Staged update and interruption recovery | Kill/restart keeps old app launchable | S156 |
| S158 | Q2 | Update rollback and paused-job compatibility | Forced failure rolls back with jobs intact | S016, S157 |

### Milestone 11 — Hardening, performance and release

| Slice | Q | Atomic deliverable | Focused test / Definition of Done | Depends |
|---|---:|---|---|---|
| S159 | Q2 | Fuzz messages, HTTP headers/ranges and filenames | 20-minute smoke; regressions saved | S019, S023, S041 |
| S160 | Q2 | Fuzz HLS/DASH parsers | 20-minute smoke; no crash/hang/OOM | S107, S110–S111 |
| S161 | Q1 | Crawler URL/HTML/CSS fuzz targets | 20-minute smoke and resource caps | S135–S140 |
| S162 | Q2 | Paired randomized IDM runner/version capture | Small A/B fixture yields valid statistics | S056 |
| S163 | Q2 | Controlled HTTP/1.1 LAN benchmark | 10 paired runs, hashes, report | S162 |
| S164 | Q2 | Controlled HTTP/2 LAN benchmark | 10 paired runs, hashes, report | S162 |
| S165 | Q2 | Non-range/single-stream parity benchmark | Reproducible overhead report | S162 |
| S166 | Q2 | One measured LAN regression profile-and-fix | Before/after report + correctness suite | S163–S165 |
| S167 | Q2 | 20/80 ms shaped-WAN benchmark | Paired throughput/retry/wire-byte report | S162 |
| S168 | Q2 | 180 ms/loss/throttle WAN benchmark | Paired adaptive-controller report | S162 |
| S169 | Q2 | One measured WAN regression profile-and-fix | Before/after report + fairness suite | S167–S168 |
| S170 | Q2 | Chrome/Edge supported-version regression | Capture/context/overlay artifact | S100, S117 |
| S171 | Q2 | Firefox stable/ESR regression | Shared contract report | S100, S117 |
| S172 | Q2 | Brave/Opera P1 regression | Results and isolated continuations | S100, S117 |
| S173 | Q2 | Progressive/HLS licensed media corpus | Hashes and ffprobe assertions | S105, S114 |
| S174 | Q2 | DASH/subtitle/live licensed media corpus | Sync/duration assertions | S114–S121 |
| S175 | Q2 | Process/worker/network fault campaign | 50 seeded runs; no corrupt complete | S032, S113, S134 |
| S176 | Q2 | Disk/DB/update interruption campaign | 50 seeded runs; invariants hold | S037, S158 |
| S177 | Q2 | Scheduled 24-hour soak workflow + 60-minute validation | Failure detection proven, then reverted | S175–S176 |
| S178 | Q1 | Review completed asynchronous soak result | Pass or one continuation per root cause | S177 |
| S179 | Q2 | Final accessibility/localization/scaling audit | Automated + keyboard primary-flow pass | S078 |
| S180 | Q2 | P0/P1 requirement-to-test evidence audit | Every mandatory ID evidenced or blocking | S159–S179 |
| S181 | Q2 | User guide, privacy, notices and troubleshooting | Links/commands verified cleanly | S149, S180 |
| S182 | Q2 | Clean-VM release-candidate journey | Install→capture→resume→media→update→uninstall | S181 |

---

## 6. Empirical continuation policy

Performance, compatibility, fuzzing and fault campaigns may reveal unknown failures. They remain quota-safe by using one root cause per continuation:

1. Preserve the failing artifact/seed/trace.
2. Add an ID under the parent, e.g. `S166A — Remove writer-lock contention`.
3. Limit it to one diagnosis/fix and one focused benchmark/regression suite.
4. If the root cause is still unknown near the quota margin, commit reproducible diagnostic instrumentation as its own slice; do not guess broadly.
5. Parent/milestone remains incomplete until mandatory continuations pass.

---

## 7. Release rule

v1 may be released only after S000–S182 and mandatory continuation slices are `DONE`, all P0/P1 requirements are traced, the IDM parity benchmark passes, browser/media matrices pass, the 24-hour soak succeeds, and the signed installer/update/rollback path is verified.
