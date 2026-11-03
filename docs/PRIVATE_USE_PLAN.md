# Private-use initial implementation plan

| Item              | Decision                                                                               |
| ----------------- | -------------------------------------------------------------------------------------- |
| Target            | This laptop only: Windows 10 Pro 22H2, build 19045, x64                                |
| Distribution      | Private per-user installation; no publication                                          |
| Session budget    | One large segment or at most two adjacent small segments per Codex 5-hour quota window |
| Initial input     | A user-pasted `http://` or `https://` URL                                              |
| Initial output    | A completed file in a user-selected local folder                                       |
| Initial installer | Unsigned per-user Windows installer for this laptop                                    |

This is a private-use track, not the public v1 release gate in
`PROGRESSIVE_IMPLEMENTATION_PLAN.md`. The existing engine contracts and tests should be reused,
but public-release work must not be pulled into a segment unless it is required to make the local
installed application work.

## Initial usable version

The initial version is complete when all of the following work on this laptop:

1. Install without administrator access.
2. Launch **Download Manager** from the Start Menu.
3. Paste one HTTP or HTTPS URL and choose a destination.
4. Start the download and see filename, status, bytes, speed, and progress.
5. Pause, resume, and cancel a download.
6. Close and reopen the app without losing durable jobs or safe partial progress.
7. Complete a download with byte-for-byte correct output.
8. Open the completed file or its containing folder.
9. Uninstall the application without deleting completed downloads.

Browser interception is useful, but it is not required for this first usable milestone. It is a
separate optional phase after the installed core application is reliable.

## Scope deliberately excluded

Do not add these items to the initial milestone:

- Code signing, SmartScreen reputation, Microsoft Store, Chrome Web Store, or Firefox Add-ons.
- Automatic updates, release channels, rollback infrastructure, or public release hosting.
- Windows 11, ARM64, clean-VM matrices, or support for other computers.
- IDM comparison benchmarks, 10 GiB acceptance runs, long soak campaigns, or release SBOM work.
- Firefox integration, browser media overlays, HLS/DASH, FFmpeg, live recording, or DRM handling.
- FTP/FTPS, site grabbing, queues, schedules, power actions, post-processing, or custom sounds.
- Telemetry, cloud accounts, diagnostic upload, or remote services.
- A general-purpose Windows service. The backend should run only for the current user and app.

If an excluded feature is found to be technically necessary for the core path, record the exact
reason before expanding scope.

## Segment sizing rules

- **Small:** normally safe to pair with the next adjacent small segment in one quota window.
- **Large:** use the whole quota window and stop after its tests and handoff notes are complete.
- Every segment starts from a clean understanding of `git status` and ends with focused tests,
  applicable fast checks, a reviewed diff, and a cohesive commit.
- A segment is not complete while its acceptance test is failing. Do not start the next segment to
  work around an incomplete dependency.
- If approximately 70% of the quota has been consumed, stop adding behavior and finish tests,
  documentation, and a reproducible handoff.

## Phase A — Local runtime

### Completion record

Phase A maps to repository completion slices S194–S195 for the required commit convention while
retaining the private-track segment names used below.

| Segment | Slice | Status | Evidence |
| ------- | ----- | ------ | -------- |
| PU01 | S194 | Complete | `pwsh -NoProfile -File scripts/check-private-use-prerequisites.ps1` verifies the pinned toolchain, Windows/Tauri prerequisites, Cargo workspace build, and TypeScript build/typecheck. |
| PU02 | S195 | Complete | `cargo test -p download-manager-cli` starts the real backend, checks authenticated health/version, rejects crash-stale discovery, restarts, and performs authenticated clean shutdown. |

The prerequisite command reports these private runtime locations but deliberately does not create
them:

| Purpose | Current-user location |
| ------- | --------------------- |
| Durable state | `%LOCALAPPDATA%\DownloadManager\State` |
| Logs | `%LOCALAPPDATA%\DownloadManager\Logs` |
| Partial files | `%LOCALAPPDATA%\DownloadManager\State\partials` |
| IPC discovery | `%LOCALAPPDATA%\DownloadManager\ipc` |
| Safe local test downloads | `%USERPROFILE%\Downloads\DownloadManager-Test` |

The test-download folder is outside the repository. Application code may create these locations
when their owning phase requires them; the Phase A prerequisite check performs no application,
browser, registry, or Start Menu mutation.

### PU01 — Verify and document the laptop build environment

**Size:** Small
**May pair with:** PU02

Work:

1. Add a private-use prerequisite check for the pinned Rust, Node.js, and pnpm versions.
2. Check the MSVC C++ build tools, Windows SDK, WebView2 Runtime, and Tauri prerequisites.
3. Report missing prerequisites with exact install guidance; do not silently install global tools.
4. Define private runtime locations beneath `%LOCALAPPDATA%\DownloadManager` for state, logs, and
   partial files.
5. Record a safe local test-download folder outside the repository.

Definition of done:

- One command reports whether this laptop can build the desktop application.
- `cargo build --workspace --locked` and the established TypeScript build/typecheck complete.
- No application, browser, registry, or Start Menu state is changed by the check.

### PU02 — Add a current-user backend executable with health IPC

**Size:** Small
**May pair with:** PU01

Work:

1. Add the missing long-running backend executable around the existing engine and local IPC code.
2. Bind only to loopback with the existing current-user credential-backed authentication.
3. Publish discovery state only after the backend is ready.
4. Support health/version and clean shutdown first; do not add download behavior in this segment.
5. Ensure stale discovery state is removed after normal shutdown and rejected after a crash.

Definition of done:

- A focused test starts the real backend, performs an authenticated health request, and shuts it
  down.
- Wrong credentials and stale discovery files do not reach backend dispatch.
- The backend runs as the current user without elevation.

## Phase B — Real download path

### PU03 — Connect add/list/status to one real HTTP(S) download

**Size:** Large

Work:

1. Connect the backend to the existing URL probe, filename, storage, and transfer code.
2. Implement the real IPC operations needed by the existing CLI: add, list, and status.
3. Run one download at a time initially; concurrency policy is not part of this segment.
4. Write to a partial file and publish the final file only after successful completion.
5. Expose bounded progress: status, downloaded bytes, total bytes when known, and failure reason.

Definition of done:

- The packaged-style CLI can add a URL through the real backend and observe it reach completion.
- Local HTTP fixture tests cover known and unknown content length, a redirect, and a network error.
- The completed bytes match the fixture hash and no failed transfer appears complete.

### PU04 — Add durable pause, resume, cancel, and restart recovery

**Size:** Large

Work:

1. Connect pause, resume, and cancel IPC operations to the real job runner.
2. Persist jobs, remote identity, events, and safe checkpoints in the existing SQLite store.
3. On app restart, recover durable states without automatically treating interrupted work as
   complete.
4. Resume only after validating the remote object and supported range behavior.
5. Keep cancel separate from deleting a completed file.

Definition of done:

- A real fixture download pauses, the backend process exits, a new process starts, and resume
  produces the exact expected hash.
- Changed remote identity refuses unsafe append.
- Cancel stops work and leaves no falsely completed output.
- CLI add/list/status/pause/resume/cancel all use the production backend path.

## Phase C — Minimal desktop application

### PU05 — Create the Tauri desktop shell and backend lifecycle

**Size:** Large

Work:

1. Add the minimal Tauri 2 application and one Windows desktop window.
2. Start or connect to the current-user backend during application startup.
3. Show a clear startup error if the backend cannot become healthy.
4. Close the connection cleanly; do not kill active work before durable state is safe.
5. Keep the shell single-user and local-only. Do not add updater, telemetry, tray, or browser code.

Definition of done:

- `tauri dev` opens one working window connected to the real backend.
- Closing and reopening the window preserves backend-owned job state.
- A focused lifecycle test covers start, already-running backend, failed start, and clean close.

### PU06 — Build the add-download and progress UI

**Size:** Large

Work:

1. Render a minimal job list using the existing desktop protocol and presentation modules.
2. Add a URL dialog with destination selection and filename preview.
3. Connect add, list, and status to the real backend instead of test fixtures.
4. Display filename, destination, status, progress, transferred bytes, total bytes, and speed.
5. Poll or subscribe at a bounded rate so the UI remains responsive.

Definition of done:

- A user can complete the initial URL-to-file flow without opening PowerShell.
- Invalid URLs and unwritable destinations are explained before transfer starts.
- Progress remains responsive during a real local fixture download.
- Existing desktop tests continue to pass and new UI-to-backend integration tests cover the flow.

### PU07 — Add essential controls and local usability

**Size:** Large

Work:

1. Wire Pause, Resume, Cancel, Open file, and Open containing folder.
2. Remember the last download directory as a current-user preference.
3. Present actionable network, disk-space, access-denied, and changed-remote errors.
4. Add a minimal confirmation before cancel/removal where data may be lost.
5. Provide a local About/Diagnostics view with product version, backend health, state path, and log
   path. Do not add upload behavior.

Definition of done:

- Every initial-version control works against a real backend job.
- Restart preserves the last directory and durable job state.
- File/folder actions target the exact completed path.
- The primary workflow is keyboard reachable at normal Windows scaling.

## Phase D — Install on this laptop

### PU08 — Build the private per-user Windows installer

**Size:** Large

Work:

1. Use the Tauri Windows bundler to produce an unsigned per-user installer, preferably NSIS.
2. Include the GUI, backend sidecar, CLI if still useful for diagnostics, UI assets, and database
   migrations.
3. Install beneath `%LOCALAPPDATA%\Programs\Download Manager` without elevation.
4. Create a Start Menu shortcut to the GUI, not the CLI.
5. Preserve the existing payload hash checks where applicable.
6. Uninstall app files and local integration, while leaving completed downloads untouched. Offer an
   explicit choice for retained application state if practical.
7. Do not package the browser native host or FFmpeg yet; neither is used by the initial milestone.

Definition of done:

- A clean temporary user-profile test installs, launches the GUI, reaches backend health, and
  uninstalls.
- Installed executables run from the final installation directory, not the build tree.
- The installer refuses corrupt or incomplete payloads.
- The artifact name includes the application version and Windows x64 target.

### PU09 — Perform the installed-laptop acceptance run

**Size:** Small
**May pair with:** none; keep this as an independent final gate

Work:

1. Run the repository fast verification checks.
2. Build the installer from a clean release build.
3. Install it for the current Windows user on this laptop.
4. Download one small fixture, one larger file, and one unknown-length fixture.
5. Exercise pause, close, relaunch, resume, cancel, open file/folder, and changed-remote refusal.
6. Reboot once with a paused job and verify safe recovery.
7. Uninstall while keeping completed downloads, then verify app-owned files and shortcuts are gone.
8. Record commands, versions, hashes, and any Windows Defender/SmartScreen warning in a bounded
   local acceptance report.

Definition of done:

- Every item in **Initial usable version** passes on this laptop.
- The installer artifact and its SHA-256 are recorded.
- Any failure becomes a narrowly scoped continuation segment; the milestone is not declared usable
  until that continuation passes.

## Optional Phase E — Chrome/Edge capture after the core works

These segments are intentionally after PU09. Direct URL downloads remain usable without them.

### PU10 — Route native-host download requests to the backend

**Size:** Large

Work:

1. Replace the production native host's current `not_ready` download response with authenticated
   forwarding to the running backend.
2. Return an acknowledgement only after exactly one durable job is accepted.
3. Preserve browser fallback when the backend is offline or rejects the request.

Definition of done:

- A native-message AddDownload request creates exactly one real job.
- Health, malformed input, wrong-origin, duplicate-request, offline, and rejection tests pass.

### PU11 — Build a locally installable Chromium extension

**Size:** Large

Work:

1. Add the Manifest V3 service-worker/runtime entry point needed to invoke the existing capture
   modules.
2. Produce a load-unpacked package for Chrome and Edge.
3. Use one deterministic local extension identity and match it in native-host registration.
4. Keep capture rules simple: enabled toggle, bypass, force, and browser fallback.
5. Do not add store packaging, Firefox, media overlays, or broad site permissions.

Definition of done:

- Chrome and Edge can load the local extension with no manifest errors.
- A fixture link is handed to the installed app exactly once.
- With the app offline, the browser download is not lost.

### PU12 — Add browser files to the private installer and test them

**Size:** Small

Work:

1. Package the native host and current-user Chrome/Edge registration with the private installer.
2. Include concise instructions for loading the unpacked local extension.
3. Verify install, repair, browser capture, browser fallback, and uninstall cleanup on this laptop.

Definition of done:

- The installed extension reports healthy native connectivity.
- One real browser click creates one app job and no duplicate transfer.
- Uninstall removes native-host registration without deleting downloads.

## Recommended quota-window sequence

| Codex window | Segment(s)  | Expected outcome                                      |
| -----------: | ----------- | ----------------------------------------------------- |
|            1 | PU01 + PU02 | Build readiness and a healthy local backend process   |
|            2 | PU03        | First real CLI-driven download                        |
|            3 | PU04        | Durable controls and restart-safe resume              |
|            4 | PU05        | Minimal desktop window connected to the backend       |
|            5 | PU06        | GUI-driven URL-to-file workflow                       |
|            6 | PU07        | Essential controls and usable errors                  |
|            7 | PU08        | Private per-user installer artifact                   |
|            8 | PU09        | Installed-laptop acceptance and initial-use milestone |
|            9 | PU10        | Optional real browser-to-backend handoff              |
|           10 | PU11        | Optional local Chrome/Edge extension                  |
|           11 | PU12        | Optional browser packaging and installed test         |

The first usable milestone therefore targets eight quota windows. It may take fewer if a large
segment proves small in the existing code, but adjacent work should not be pulled forward until the
current segment is complete and tested.

## Request format for future Codex sessions

Start each implementation session with a narrowly scoped request such as:

> Implement PU03 from `docs/PRIVATE_USE_PLAN.md`. Do not start PU04. Run the focused tests and the
> applicable fast checks, update the plan/status notes with evidence, and commit the completed
> segment following the repository commit instructions.

For the first window only, request `PU01 and PU02` together. For every large segment, request only
one segment.
