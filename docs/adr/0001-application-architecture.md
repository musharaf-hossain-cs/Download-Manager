# ADR 0001: Application architecture and dependency policy

- Status: Accepted
- Date: 2026-10-25
- Decision owners: Download Manager maintainers
- Scope: Milestone 0 architecture baseline

## Context

Download Manager needs a crash-safe high-throughput transfer core, a Windows desktop interface, Chromium and Firefox integration, and a constrained media toolchain. The core must remain testable without a browser or GUI, browser-originated input must cross a narrow trust boundary, and later performance work must be measurable against the benchmark in `REQUIREMENTS.md`.

## Decision

The application uses these process and package boundaries:

```text
Chromium / Firefox WebExtension
        │ length-prefixed, versioned native messages
        ▼
Rust native host ── authenticated local IPC ── Rust engine process
                                                  │
                    Tauri 2 desktop shell ─────────┤
                                                  ├── SQLite + partial files
                                                  └── restricted FFmpeg worker
```

- Stable Rust with Tokio will own transfer scheduling, persistence, native messaging, and the CLI. Milestone 0 contains dependency-free Rust boundaries; Tokio and persistence libraries are introduced only by the slices that need them.
- The engine is the authority for job state. Desktop and extensions are clients and cannot write the database or partial files directly.
- The Tauri 2 shell will host a TypeScript UI and communicate over a versioned, typed local protocol. It will not contain transfer logic.
- Chromium Manifest V3 and Firefox builds will share TypeScript domain code but keep browser-specific manifests and adapters.
- SQLite will hold durable metadata. Download bytes remain in explicitly managed files with independently checkable checkpoints.
- FFmpeg/ffprobe will run out of process with explicit resource, input, protocol, and time limits. DRM/license acquisition is outside the system boundary.
- All page, URL, header, native-message, media-manifest, and IPC data is untrusted until it passes size, type, scheme, and semantic validation.

## Dependency policy

1. Prefer the standard library and existing workspace dependencies. Every new runtime dependency needs a concrete requirement, maintained upstream, compatible license, and an owner.
2. Pin developer toolchains and direct JavaScript dependencies. Commit both Rust and pnpm lockfiles.
3. GitHub Actions are referenced by immutable commit SHA with a version comment. Workflow permissions default to read-only.
4. CI blocks known high-severity dependency advisories, disallowed licenses/sources, and detected secrets. Exceptions require an expiry, owner, rationale, and linked issue.
5. Rust allows crates.io and the public GitHub source only; unknown registries and git sources are denied by default. JavaScript production install scripts require explicit review.
6. Generated/native binaries are never accepted solely from an unverified download. Release binaries and updates will be signed in the packaging milestone.
7. Dependencies that parse hostile formats or cross a process boundary receive priority fuzzing and patch review.

## Consequences

The engine can be benchmarked and fault-tested without UI variability, and browser compromise does not directly expose storage APIs. The design introduces IPC/versioning work and multiple build targets, but those costs are necessary for isolation and independent testing. Tauri and FFmpeg remain provisional until their dedicated build and media slices validate packaging, accessibility, and sandbox behavior.

## Alternatives considered

- A TypeScript-only Electron application would simplify language boundaries but expands the trusted renderer/runtime surface and makes a low-overhead native transfer core less direct.
- In-process browser or packet hooks would increase capture breadth but conflict with least privilege, browser-store policy, and the product's explicit integration boundary.
- Embedding FFmpeg libraries in the engine would reduce process startup cost but would put complex hostile-media parsing in the core trust boundary.

## Review checklist

- [x] Engine, desktop, extension, native-host, persistence, and media-worker ownership is explicit.
- [x] Trust boundaries and untrusted inputs are identified.
- [x] Crash safety and benchmark isolation remain possible.
- [x] Chromium and Firefox can share contracts without sharing manifests.
- [x] DRM and access-control bypass remain excluded.
- [x] Dependency, license, advisory, source, action-pin, and exception policies are defined.
- [x] The sample Rust and TypeScript packages build and test independently.
- [x] The decision aligns with the P0 security and data requirements.

