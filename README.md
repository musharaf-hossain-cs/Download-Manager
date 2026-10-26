# Download Manager

A planned high-speed, local-first download manager for Windows with IDM-class core capabilities: adaptive dynamic segmentation, safe resume, browser integration, in-player media detection, queues, scheduling, categories, site grabbing, proxy/authentication support and command-line automation.

> **Project status: Milestone 4 in progress through S065.** The typed desktop shell, validated Add URL flow, acknowledged single-job controls, exact-target destructive actions, and mixed-state bulk resume/stop/delete-completed controls are implemented. Segment details begin at S066.

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

The accepted baseline uses a Rust download engine and native host, a Tauri desktop shell, SQLite state, TypeScript WebExtensions and a restricted FFmpeg-based media worker. The process boundaries and dependency rules are recorded in [`ADR 0001`](./docs/adr/0001-application-architecture.md).

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

Contributors work through the slices in [`PROGRESSIVE_IMPLEMENTATION_PLAN.md`](./PROGRESSIVE_IMPLEMENTATION_PLAN.md) in order and keep the repository buildable and tested after every slice. Current state and evidence live in [`IMPLEMENTATION_STATUS.md`](./IMPLEMENTATION_STATUS.md).

Expected quality gates after the workspace bootstrap phase:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm lint
pnpm test
pnpm typecheck
```

Run every fast local gate on Windows with:

```powershell
./scripts/verify.ps1
```

The repository pins Rust 1.99.1, Node.js 24.22.1 LTS, and pnpm 12.15.0. Security tooling and its pinned CI versions are documented by [the security workflow](./.github/workflows/security.yml).

## Current repository contents

```text
apps/                 TypeScript desktop boundary
crates/               Rust engine, CLI and native-host crates
docs/                 Architecture decisions and traceability format
extensions/           Shared browser-extension boundary
scripts/              Local quality and policy gates
.github/workflows/    Windows CI and security policy
```

## License

No open-source license has been granted yet. The repository is publicly readable, but reuse, modification and redistribution rights are reserved until a `LICENSE` file is intentionally added.
