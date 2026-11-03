# Third-party notices

Last updated: 2026-11-03

Download Manager contains or depends on third-party software. Copyright remains with each upstream
author. This notice is informational and does not replace the license text shipped with a release.

## Direct runtime components

The Rust runtime directly uses the following crates. Locked metadata can enumerate transitive
dependencies; a production release must include their generated license inventory:

| Component | Purpose | Declared license |
|---|---|---|
| chrono, chrono-tz | Time and time-zone handling | MIT OR Apache-2.0 |
| ed25519-dalek | Signed update verification | BSD-3-Clause |
| fs2 | File locking/filesystem support | MIT OR Apache-2.0 |
| httpdate | HTTP date parsing | MIT OR Apache-2.0 |
| md-5, sha1, sha2 | User-requested/integrity hashes | MIT OR Apache-2.0 |
| percent-encoding, url | URL processing | MIT OR Apache-2.0 |
| quick-xml | XML/media manifest parsing | MIT |
| reqwest | HTTP client | MIT OR Apache-2.0 |
| rusqlite / SQLite | Durable state | MIT / Public Domain blessing |
| scraper | Bounded HTML discovery | ISC |
| semver | Version validation | MIT OR Apache-2.0 |
| serde, serde_json | Versioned data contracts | MIT OR Apache-2.0 |
| thiserror | Error contracts | MIT OR Apache-2.0 |
| uuid | Stable identifiers | MIT OR Apache-2.0 |
| keyring-core, windows-native-keyring-store | Windows credential storage | MIT OR Apache-2.0 |

The browser and desktop TypeScript delivered by this repository have no declared production npm
dependencies. Build, test, packaging, and lint dependencies are development tools and are still
covered by the repository's license audit.

## Optional/external components

Clear media muxing or probing may invoke an external FFmpeg/ffprobe executable. FFmpeg is licensed
under LGPL-2.1-or-later by default, with optional configurations under GPL terms. The exact license
depends on the binary's build configuration. This repository does not grant an FFmpeg license; a
distributor that bundles it must provide the corresponding build/license notices and comply with
the applicable source and relinking obligations.

Chrome, Chromium, Edge, Firefox, Brave, Opera, Windows, Widevine, PlayReady, FairPlay, and other
names are trademarks of their respective owners. Mention does not imply sponsorship or affiliation.

## Reproduce the inventory

From a clean repository checkout using the pinned toolchains:

```powershell
cargo metadata --format-version 1 --locked
pnpm audit:licenses
powershell -File scripts/security-quality.ps1
```

`Cargo.lock` and `pnpm-lock.yaml` are the authoritative version locks for a source build. The
security gate checks Rust/Node advisories, allowed dependency sources, and declared licenses. Release
packaging must preserve the generated complete inventory and all license texts required by the exact
artifacts it ships.

## Download Manager repository license

No open-source license has been granted for Download Manager itself. The repository is publicly
readable, but reuse, modification, and redistribution rights remain reserved until a `LICENSE` file
is intentionally added. Third-party licenses apply only to their respective components and do not
license this repository's original code.
