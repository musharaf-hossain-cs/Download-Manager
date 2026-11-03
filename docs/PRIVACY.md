# Privacy statement

Last updated: 2026-11-03

Download Manager is designed as a local-first Windows application. This statement describes the
repository release-candidate behavior; a distributor must disclose any additional store, signing,
update-host, or support service it operates.

## Data processed locally

The app may store download URLs and sanitized remote identity, filenames and destinations, byte and
segment checkpoints, queue/category/schedule settings, progress and state events, browser/profile
labels, media track choices, retry/error results, and redacted operational logs. Settings, history,
and partial state are kept under `%LOCALAPPDATA%\DownloadManager`; installed programs live under
`%LOCALAPPDATA%\Programs\Download Manager`. Completed downloads are stored only at destinations the
user selects.

HTTP/proxy credentials and other reusable secrets are stored through the current Windows user's
credential protection rather than in the SQLite database or logs. Request context is bounded,
expires, and is redacted before durable logging or portable export.

## Network activity

Network requests occur when the user or an enabled rule probes or downloads a URL, a scheduled job
runs, a configured proxy/authentication challenge is answered, clear media manifests/fragments are
retrieved, or an update check is requested/enabled. Requests go to the selected source and its
redirects, the configured proxy, and the configured signed-update source. Browser handoff uses a
current-user native-messaging channel on the same computer.

The repository implementation has no analytics, advertising identifier, or sale of personal data.
There is no automatic diagnostics upload. A distributor adding telemetry must keep it opt-in and
publish its endpoint, fields, retention, and withdrawal behavior before release.

## Browser and media data

The extension evaluates eligible link and clear-media candidates needed for capture. Global and
per-site rules determine whether a handoff is attempted. Incognito/private capture is off by
default; when explicitly enabled, private metadata is kept ephemeral and is not persisted as normal
profile history.

DRM/EME license requests, keys, PSSH/KID material, and protected-media secrets are intentionally not
captured, followed, decrypted, or exported. Protected content is classified as unsupported before
transfer selection.

## Diagnostics, logs, and exports

Diagnostics are generated locally. The complete sanitized preview is shown before an explicit
no-overwrite export. Authorization and proxy-authorization values, cookies, passwords, API keys,
tokens, URL/proxy user information, and common signed-URL credentials are redacted before preview,
logs, database audit records, or export. Redaction reduces exposure but users should still review a
bundle before sharing it.

The app does not transmit an exported diagnostic or settings/history archive. The user controls its
destination and any later sharing. Imports require a preview and revision-bound confirmation before
merge or replacement.

## Retention and deletion

Completed history and event retention follows the user's settings; active jobs and partials are not
age-deleted as completed history. Removing a history record does not delete its completed file.
Deleting a file is a separate exact-target action.

During uninstall, `Keep` retains settings, history, and partial data; `Remove` deletes those app-data
items. Both remove native-host registrations and installed application files. Completed downloads
outside app data remain in place. Users can also clear completed history from the app without
deleting downloaded files.

## Security and user choices

Native messages are size/version/origin checked, URLs are restricted to supported schemes, paths are
sanitized, and update metadata/artifacts must pass signature and hash verification. These controls
do not make every downloaded file safe: use antivirus scanning and open untrusted content with
appropriate isolation.

Users can disable browser capture globally or per site, keep private capture off, disable clipboard
detection and media overlays, review diagnostics before export, select retention, and choose
Keep/Remove during uninstall. A distributor that adds an automatic update-check schedule must also
provide an explicit enable/disable choice.

Questions or security reports should use the support/security contact published by the distributor
of the signed build. Do not attach credentials, cookies, private URLs, or unreviewed diagnostics.
