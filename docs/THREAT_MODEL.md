# Download Manager threat model

- Status: Active release gate
- Security owner: Download Manager maintainers
- Last reviewed: 2026-11-03
- Review cadence: Every release and after a trust-boundary, parser, updater, or IPC change
- Scope: Desktop shell, engine, CLI, browser extensions/native host, persistence, updater, and media worker

## Security objectives

1. Only the current Windows user's trusted clients can ask the engine to mutate download state.
2. Untrusted network, webpage, extension, archive, filename, and media input cannot escape its bounded parser or chosen destination.
3. Download and update integrity failures fail closed and cannot become installed or completed artifacts.
4. Credentials, signed URLs, cookies, and authorization material do not reach ordinary databases, logs, exports, or diagnostics.
5. A parser or helper compromise cannot silently become arbitrary engine or user-level command execution.

The engine is the authority for jobs and filesystem changes. The desktop, CLI, native host, extension, download server, update service, and media worker are separate trust principals even when they run for the same user. Windows administrators and compromise of the signed release infrastructure are outside the protection boundary; they remain supply-chain and operational risks managed by signing, protected CI identities, and incident response.

## Trust boundaries and data flow

```text
Untrusted webpage
  -> least-privilege extension
  -> length/schema/origin checked native messaging host
  -> current-user authenticated local IPC
  -> engine authority
       -> canonical destination + partial files
       -> SQLite metadata (no plaintext secrets)
       -> TLS-validating download/update servers
       -> bounded low-privilege media worker
```

Every arrow is a validation boundary. Authentication establishes the caller's principal; it never makes page-derived URLs, headers, filenames, manifests, or downloaded bytes trusted.

## Threat register

| ID    | Threat actor / capability                                                                                                                                                                                 | Assets and entry points                                                                                    | Required controls and verification                                                                                                                                                                                                                                                                                       | Residual risk and fail-safe behavior                                                                                                                                                                                                                   | Review owner              |
| ----- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------- |
| TM-01 | Malicious webpage can submit crafted links, names, referrers, headers, very large batches, or repeated requests through a permitted extension.                                                            | Native-message framing, extension request schema, URL/job creation, browser request context.               | Least browser permissions; exact packaged extension-ID and sender checks; strict versioned schema; byte/item limits; HTTP(S)-only URL types; replay-resistant request IDs/nonces; header CR/LF rejection; secret redaction and expiry. Fuzz native framing and page-derived strings.                                     | A browser with the user's explicit host permission can request a legitimate download. The engine still owns destination and execution policy; malformed/oversized/replayed input is rejected without a job.                                            | Browser integration owner |
| TM-02 | Malicious download server controls redirects, status/length/range headers, filenames, content bytes, timing, connection failures, and authentication challenges.                                          | Probe, redirect, proxy/auth, segmented transfer, resume identity, completion and checksum paths.           | Platform TLS validation; bounded redirect/downgrade policy; strict `Content-Range` and length checks; remote-identity validation before resume; positional writes; retry budgets; safe filename selection; exact final-size/hash checks; completion only after durable verification.                                     | A server may lie consistently or serve harmful but structurally valid content. The product preserves integrity evidence and never auto-opens it; transport or identity ambiguity pauses/fails rather than claiming completion.                         | Transfer engine owner     |
| TM-03 | Compromised extension message is syntactically valid but carries confused-deputy commands, another origin/extension identity, replayed IDs, secrets, or a request outside the browser bridge's authority. | Browser native host and extension-to-engine handoff.                                                       | Bind launch origin to exact packaged extension allowlist; current-user check; nonce and bounded replay cache; allowlist message variants; no shell/CLI invocation; engine revalidates URLs, filenames, job actions, and policy.                                                                                          | Compromise of an allowlisted extension can submit downloads the user could submit, but cannot directly access the database, arbitrary filesystem APIs, updater, or command execution. Rejected identity/replay never reaches engine authority.         | Native-host owner         |
| TM-04 | Local unprivileged process or a different Windows account probes IPC endpoints, copies a discovery file, replays traffic, or attempts to control another user's engine.                                   | CLI/native-host/desktop local IPC and endpoint discovery state.                                            | Loopback-only endpoint; random bearer secret held in Windows Credential Manager for the current user; discovery file contains no secret; bounded strict protocol; per-instance identifier; constant-time authentication before dispatch; no command strings or shell expansion.                                          | Malware already running as the same Windows user shares that user's security boundary and may access user-authorized resources. A different unprivileged account cannot retrieve the vault secret; authentication failure invokes no backend action.   | IPC/engine owner          |
| TM-05 | Path traversal input uses `..`, absolute/device/UNC paths, alternate data streams, reserved names, NUL/control characters, archive entries, Unicode ambiguity, or symlink/reparse races.                  | Suggested filenames, content disposition, user destination, archive preview/extraction, final rename/move. | Treat remote names as one component; sanitize Windows reserved names/ADS/control characters; canonicalize and bind destructive confirmation to the engine-derived exact target; keep temporary/final operations within chosen roots; reject archive entries escaping the root; avoid following untrusted reparse points. | A user may explicitly select a sensitive writable directory. Policy warns or rejects unsafe targets; an ambiguous or changed target aborts without deleting or overwriting an unrelated path.                                                          | Storage/filesystem owner  |
| TM-06 | Update tampering changes metadata, artifact bytes, version/channel, activation state, or rollback material in transit, at rest, or through a compromised mirror.                                          | Release metadata, download/staging directory, signature/hash verification, activation and rollback.        | Pinned Ed25519 metadata verification over exact bytes; expected hash/length/name; stable-channel and version policy; code-signed installer/executables; stage then reverify before activation; atomic activation with rollback; CI advisory/license/secret gates.                                                        | Theft of an authorized signing key or protected CI identity can produce trusted malicious updates. Release owners rotate/revoke keys and stop promotion; any ordinary mismatch blocks install and retains the prior version.                           | Release/security owner    |
| TM-07 | Media parser exploit uses hostile HLS/DASH/XML/subtitle/container input, decompression/resource exhaustion, protocol tricks, or FFmpeg vulnerabilities to escape parsing.                                 | Manifest parsers, segment fetch, subtitle conversion, ffprobe/FFmpeg worker and generated output.          | Bounded data-only parsers; scheme/host/size/count/time limits; DRM/key/license paths ignored; maintained pinned tools; FFmpeg/ffprobe out of process with no shell, allowlisted arguments/protocols, restricted environment, resource/time limits, and kill-on-timeout; fuzz parser boundaries.                          | OS-level sandbox coverage and third-party parser vulnerabilities can vary by packaged tool/version. Worker failure is treated as a failed media job; no output is trusted or opened automatically, and a parser advisory blocks release until triaged. | Media/security owner      |

## Cross-cutting abuse cases

- Secrets are stored in the current-user OS vault and represented elsewhere only by non-secret references. Logs and exports redact authorization, cookies, proxy passwords, tokens, and signed URL parameters before persistence.
- Network content is data. No downloaded file, archive member, scanner result, media metadata, custom header, queue name, or post-processing value becomes a shell command.
- Denial of service remains possible from a hostile server, permitted webpage, same-user process, or very large legitimate workload. Message, parser, connection, retry, disk, and time budgets bound damage; cancellation and durable recovery remain available.
- Privacy failures are security failures: telemetry/crash reporting is opt-in, captured fields are previewable, and ordinary operation stays local except for user-requested network traffic and update checks.

## Release review procedure

For every release, the security owner records the following in the release review:

1. Re-run the focused tests named by the traceability rows for SEC-001 through SEC-014 and inspect any changed trust boundary.
2. Review new dependencies, parsers, protocols, privileged helpers, browser permissions, IPC methods, and update keys against this register.
3. Assign an owner and expiry to each accepted security exception. A missing owner, missing expiry, plaintext secret, unauthenticated authority path, signature bypass, or parser escape is release-blocking.
4. Update the relevant threat row when capability, mitigation, residual risk, or ownership changes; update `Last reviewed` only after this review is complete.

## Assumptions and exclusions

- The Windows kernel, current user's credential vault, platform TLS roots, and correctly configured code-signing verification are trusted computing base components.
- DRM circumvention, credential theft, universal media capture, stealth monitoring, and execution of downloaded programs are not product capabilities.
- Administrator/root compromise, malicious firmware/kernel drivers, physical attacks on an unlocked session, and compromise of authorized signing keys cannot be fully prevented by this application. Detection, revocation, rebuild, and incident response are required.
