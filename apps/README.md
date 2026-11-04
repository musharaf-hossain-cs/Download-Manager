# Desktop applications

Desktop application packages live in this directory.

## Private-use desktop shell

The Tauri 2 shell is in `apps/desktop`. From the repository root, run:

```powershell
pnpm --filter @download-manager/desktop tauri dev
```

The development hook builds the current-user backend beside the desktop executable. The shell
connects to an already-running healthy backend when available and otherwise starts it. Closing the
window releases the desktop connection without shutting down backend-owned downloads.

The window accepts an HTTP or HTTPS URL, uses the native Windows folder picker, previews the
resolved filename and output path, and starts the production backend download. The job list polls
at a bounded rate and shows state, progress, transferred/total bytes, speed, and ETA.
