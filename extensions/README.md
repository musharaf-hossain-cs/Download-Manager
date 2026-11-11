# Browser extensions

Shared and browser-specific WebExtension packages live in this directory.

## Local Chrome and Edge package

Build the Manifest V3 load-unpacked directory from the repository root:

```powershell
pnpm --filter @download-manager/browser-extension build:chromium
```

Then open `chrome://extensions` or `edge://extensions`, enable **Developer mode**, choose
**Load unpacked**, and select `extensions/browser/dist/chromium`. Both browsers must report the
extension ID `ackbonlbpniilcfmmnfdohnhbdbajhii`; a different ID means the package key and native
host registration do not match.

Clicking the extension action toggles automatic download capture. **Download with Download
Manager** forces one link to the app even while automatic capture is off, while **Download in
browser** bypasses capture for that link. The original browser transfer continues unchanged when
the native app is unavailable or rejects a request.

For repository development, build the native host and register it for the current user:

```powershell
cargo build -p download-manager-native-host
pwsh -NoProfile -File scripts/native-host-dev.ps1 -Action Register `
  -HostPath target/debug/download-manager-native-host.exe -Browser Chromium
```

Remove the development registration when finished:

```powershell
pwsh -NoProfile -File scripts/native-host-dev.ps1 -Action Unregister -Browser Chromium
```

