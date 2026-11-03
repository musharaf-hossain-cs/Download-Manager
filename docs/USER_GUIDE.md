# Download Manager user guide

This guide describes the Windows release-candidate workflow. Download Manager is for files and
clear (non-DRM) media that you are authorized to download. It does not bypass paywalls, access
controls, Widevine, PlayReady, FairPlay, EME, or encrypted-media licenses.

## System requirements

- 64-bit Windows 10 or Windows 11, running as a standard user.
- A supported Chrome, Edge, or Firefox release for browser capture.
- Enough free space for the final file and its partial data. FAT/exFAT and removable destinations
  may not provide the same atomic replacement guarantees as NTFS/ReFS.
- Network and proxy credentials, when required by the source, supplied by the user.

## Install and connect a browser

1. Obtain the versioned Windows x64 release package from the release publisher and verify the
   publisher's signature before opening it. Do not install a package whose signature or hash check
   fails.
2. Extract the package to a new directory and run its per-user installer from PowerShell:

   ```powershell
   ./install.ps1
   ```

   The app is installed under `%LOCALAPPDATA%\Programs\Download Manager`; no administrator access
   is required. The installer verifies every packaged payload before moving it into place.
3. Install the matching Download Manager browser extension from the browser-specific package or
   approved store listing supplied with the release. Review its link-capture, context-menu, and
   site-access permissions before enabling it.
4. Open the extension health view. It should show the native host as connected and the same
   protocol/product version. Private-window capture is off by default.

If the health check reports a missing or corrupt native host, close every browser and run:

```powershell
& "$env:LOCALAPPDATA\Programs\Download Manager\native-host-registration.ps1" -Action Repair
```

Then reopen the browser and retry the health check. The repair is current-user only and recreates
the Chrome, Edge, and Firefox host registrations.

## Add and control downloads

- Paste or drop an `http://` or `https://` URL into the add dialog. Review the probed filename,
  destination, size, category, queue, and start-now/start-later choice before confirming.
- For browser capture, click an eligible link. The browser cancels its own download only after the
  native app acknowledges exactly one job. If the app is unavailable or rejects the request, use
  the browser's normal download fallback.
- Use **Force capture** (Ctrl by default) or **Bypass capture** (Alt by default) while clicking.
  Bypass wins if both are pressed. Change conflicting keys in extension settings.
- Use the link context menu for one link, selected links, or all links. Review and filter a batch
  before queuing it.
- Pause preserves the last durable safe offset. Resume revalidates the remote identity before
  appending; if the server object changed or no longer supports the required range, restart rather
  than combining different bytes.
- For a ZIP with a known length and confirmed range support, preview lists at most 2,048 classic
  single-disk entries from a bounded central directory without downloading the whole archive.
  ZIP64, multi-disk, malformed, or uncooperative servers show preview unavailable.
- Cancel stops active work. Removing a history record and deleting a completed file are separate
  actions and require the exact target to be confirmed.

Use only destinations you control. A low-space or access-denied error pauses safely; free space or
choose another writable destination before resuming.

## Media downloads

For detected progressive MP4/WebM/audio or clear HLS/DASH media, use **Download media** on the
player and choose the desired quality, language, codec, audio, and subtitle tracks. Separate audio
and video tracks may require an FFmpeg/ffprobe worker supplied or approved with the release.

Protected or unsupported media is reported as such and is not sent to a key, license, or decryption
path. Signed URLs can expire: refresh them only from the original browser profile/session. Resume
continues only when the refreshed manifest describes the same media object and topology.

## Queues, schedules, and limits

- Move jobs into named queues, order them, set a per-queue concurrency cap, and start or stop the
  queue explicitly.
- A schedule uses its configured time zone. Review daylight-saving and missed-run policy before
  relying on an unattended weekly run.
- Global/per-job speed limits, per-host connection caps, metered-network policy, battery policy,
  and low-disk thresholds can reduce or pause activity. A user pause always takes precedence over
  an automatic resume.
- A server rule matches one exact hostname and can override that host's connection cap,
  user-agent and non-sensitive request headers, disable segmentation, or select system, direct, or
  manual HTTP(S)/SOCKS4/SOCKS5 proxy routing. SOCKS5 and HTTP proxy passwords stay in the current
  user's Windows credential vault; durable rules contain only a credential reference. Subdomains
  do not inherit a parent rule. Authorization, cookie, proxy-authorization, framing, range, and
  host headers are not accepted as durable overrides.

## Command line

The packaged executable exposes a versioned automation contract. Job commands use bounded
loopback IPC authenticated by a random secret in the current Windows user's credential vault; the
discovery file contains no secret. The connected release backend must be available;
transport-unavailable is a distinct error, not a successful queue operation.

```powershell
download-manager.exe --help
download-manager.exe add "https://example.test/file.zip" --output "C:\Downloads" --wait --json
download-manager.exe list --json
download-manager.exe status JOB_ID --json
download-manager.exe pause JOB_ID --json
download-manager.exe resume JOB_ID --json
download-manager.exe cancel JOB_ID --json
download-manager.exe queue start "Night Queue" --json
```

Use `--later` to queue without starting, `--queue NAME` to select a queue, `--name FILENAME` to
request a name, `--quiet` for successful add/queue operations, and `--timeout SECONDS` only with
`--wait`. JSON output includes `contract_version`; automation must reject unsupported versions and
handle nonzero exit codes.

## Diagnostics and privacy

The diagnostics preview is assembled locally and redacted before display or export. Review the
entire preview, choose a new destination, and share the resulting file yourself; the app has no
automatic diagnostic-upload path. See the [privacy statement](./PRIVACY.md) for stored data and
network behavior, and [third-party notices](./THIRD_PARTY_NOTICES.md) for components and licenses.

## Troubleshooting

### The extension says disconnected

1. Confirm the desktop app and extension came from the same release.
2. Close all browser processes, run the repair command from **Install and connect a browser**, and
   reopen the browser.
3. In the extension self-check, verify the expected extension identity, native-host manifest path,
   executable path, and protocol version. Reinstall only the component identified as mismatched.

### A click was not captured

Check that capture is enabled globally and for the current site, private-mode capture is explicitly
enabled if applicable, the file type/known size passes the rule, and Alt is not held. Use force
capture or the context menu only for a valid HTTP(S) link. If the app is offline, keep the browser
fallback rather than retrying until duplicate jobs are created.

### Resume is refused

The ETag, Last-Modified value, size, range support, signed URL identity, or media topology changed.
This refusal protects the partial file. Restart as a new download or obtain a fresh authorized URL;
do not rename or splice partial files manually.

### A download is slow or throttled

Check app/job speed limits, queue priority, per-host caps, proxy/VPN, battery/metered policy, server
rate limits, disk pressure, and retry responses such as 429/503. More connections do not override a
server cap or network congestion. Compare only reproducible, hash-equal runs.

### Media is missing or will not mux

Confirm the player is using clear progressive, HLS, or DASH media and that the overlay is enabled
for the site. Protected media is intentionally unsupported. For clear media, inspect the selected
tracks, available disk space, FFmpeg/ffprobe availability, worker timeout, and retained original
tracks. A worker failure must not delete its input tracks.

### Disk, path, antivirus, or post-processing error

Free space, use a writable local destination, and avoid Windows device names or alternate data
stream syntax. For antivirus attention, inspect the scanner result before opening the file. A
post-processing tool must be explicitly allowlisted; its failure does not make the download itself
complete or safe to open.

### Export a support bundle

Open diagnostics, generate a fresh preview, read every section, then export to a new file. The app
refuses overwrite and redacts authorization headers, cookies, passwords, proxy credentials, tokens,
signed-URL secrets, and URL user information. Add a short reproduction description separately.

## Uninstall

Run the installed uninstaller and make an explicit data choice:

```powershell
& "$env:LOCALAPPDATA\Programs\Download Manager\uninstall.ps1" -UserData Keep
& "$env:LOCALAPPDATA\Programs\Download Manager\uninstall.ps1" -UserData Remove
```

Run exactly one command. `Keep` preserves settings, history, and partials; `Remove` deletes those app
data items. Both choices remove the app, native-host registrations, manifests, onboarding state, and
Start Menu shortcut. Completed downloads outside app data are never deleted by the uninstaller.
