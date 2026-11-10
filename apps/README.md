# Desktop applications

Desktop application packages live in this directory.

## Private-use desktop shell

The Tauri 2 shell is in `apps/desktop`. From the repository root, run:

```powershell
pnpm --filter @download-manager/desktop tauri dev
```

Build the private-use unsigned per-user NSIS installer and its SHA-256 record on the pinned Windows
x64 laptop with:

```powershell
pwsh -NoProfile -File scripts/build-private-use-installer.ps1
```

The focused PU08 smoke installs the artifact, launches the installed GUI/backend, checks
authenticated health and payload corruption refusal, and uninstalls while preserving retained
state and completed downloads:

```powershell
pwsh -NoProfile -File scripts/test-private-use-installer.ps1
```

PU09's deterministic loopback fixture server supports the final installed-laptop acceptance run:

```powershell
node scripts/private-use-acceptance-fixture.mjs --port 49321
```

The completed private milestone evidence is recorded in
`docs/evidence/s202-private-use-acceptance.json`.

The development hook builds the current-user backend beside the desktop executable. The shell
connects to an already-running healthy backend when available and otherwise starts it. Closing the
window releases the desktop connection without shutting down backend-owned downloads.

The window accepts an HTTP or HTTPS URL, uses the native Windows folder picker, previews the
resolved filename and output path, and starts the production backend download. The job list polls
at a bounded rate and shows state, progress, transferred/total bytes, speed, and ETA.
