# Download Manager

A planned high-speed, local-first download manager for Windows with IDM-class core capabilities: adaptive dynamic segmentation, safe resume, browser integration, in-player media detection, queues, scheduling, categories, site grabbing, proxy/authentication support and command-line automation.

> **Project status: requirements and implementation planning.** No application binary is available yet.

## Goals

- Match or exceed the current stable Internet Download Manager baseline in controlled, repeatable throughput benchmarks.
- Preserve download correctness through pause, crash, reboot, network changes and recoverable server failures.
- Integrate with Chrome, Edge and Firefox through WebExtensions and a validated native-messaging bridge.
- Detect and download supported progressive and non-DRM HLS/DASH media, including quality selection, subtitles and separate audio/video muxing.
- Remain local-first, privacy-conscious and secure by default.

The speed goal is a measured release gate, not a claim that any client can override server caps, congestion, storage limits or site policy.

## Documentation

- [Product Requirements and SRS](./REQUIREMENTS.md)
- [Progressive Implementation Plan](./PROGRESSIVE_IMPLEMENTATION_PLAN.md)

The implementation plan divides the project into small quota-safe slices. Each slice is designed to fit comfortably inside one Codex 5-hour usage window even when intensive work consumes that allowance in roughly 1–2 hours. Each slice includes focused tests, documentation and a commit. Long-running release tests such as the 24-hour soak are automated separately.

## Planned architecture

The provisional architecture uses a Rust download engine and native host, a Tauri desktop shell, SQLite state, TypeScript WebExtensions and a restricted FFmpeg-based media worker. The first implementation phase must validate and record these choices in architecture decision records before feature work begins.

```text
Browser extensions ──native messaging──► Native host / engine
                                              │
Desktop UI ◄────────typed local IPC───────────┤
                                              ├── SQLite/checkpoints
                                              ├── HTTP transfer workers
                                              └── restricted media worker
```

## Safety and content policy

This project is intended for content the user is authorized to download. It will not bypass DRM/EME systems such as Widevine, PlayReady or FairPlay; defeat paywalls or access controls; extract license keys; or promise universal downloading from YouTube or any other service. Users must follow applicable law, site terms and rights-holder permissions.

## Development workflow

Development has not started. When it does, contributors should work through the phases in [`PROGRESSIVE_IMPLEMENTATION_PLAN.md`](./PROGRESSIVE_IMPLEMENTATION_PLAN.md) in order and keep the repository buildable and tested after every slice.

Expected quality gates after the workspace bootstrap phase:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm lint
pnpm test
pnpm typecheck
```

The exact supported toolchain and commands will be pinned in the repository during Phase 01–02.

## Current repository contents

```text
README.md
REQUIREMENTS.md
PROGRESSIVE_IMPLEMENTATION_PLAN.md
```

## License

No open-source license has been granted yet. The repository is publicly readable, but reuse, modification and redistribution rights are reserved until a `LICENSE` file is intentionally added.
