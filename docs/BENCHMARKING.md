# Paired performance benchmark

`download-manager-benchmark` runs an external baseline (IDM in the release lab) and the
Download Manager candidate in seeded, randomized A/B order. It captures both version-command
outputs, records every pair and output hash, and reports median, p10, p90, coefficient
of variation, and paired goodput differences.

Run it with a versioned JSON plan and a report prefix:

```powershell
cargo run --locked -p download-manager-engine --bin download-manager-benchmark -- `
  .\benchmark-plan.json .\test-results\lan-http1
```

The command writes `lan-http1.json` and `lan-http1.md`. A plan uses this shape:

```json
{
  "schema_version": 1,
  "scenario": "controlled-lan-large-file",
  "protocol": "HTTP/1.1",
  "seed": 5331243,
  "pairs": 10,
  "source": "http://127.0.0.1:8080/file.bin?pair={pair}&seed={seed}",
  "output_directory": "test-results/lan-http1-output",
  "retain_outputs": false,
  "baseline": {
    "name": "IDM",
    "version_command": {
      "program": "C:/lab/run-idm-download.exe",
      "arguments": ["--version"]
    },
    "download_command": {
      "program": "C:/lab/run-idm-download.exe",
      "arguments": ["--url", "{url}", "--output", "{output}"]
    }
  },
  "candidate": {
    "name": "Download Manager",
    "version_command": {
      "program": "C:/lab/run-download-manager.exe",
      "arguments": ["--version"]
    },
    "download_command": {
      "program": "C:/lab/run-download-manager.exe",
      "arguments": ["--url", "{url}", "--output", "{output}"]
    }
  }
}
```

Command arguments are passed directly without shell evaluation. The runner replaces `{url}`,
`{output}`, `{pair}`, and `{seed}`. The same expanded URL is used for both sides of a pair, so a
fixture can implement deterministic cache busting while preserving paired conditions. Benchmark
outputs are deleted after their size and SHA-256 are recorded unless `retain_outputs` is true.

## Controlled LAN scenarios

HTTP/1.1 and HTTP/2 controlled-LAN reports are accepted only when they contain at least 10 paired
runs and every baseline/candidate output has the same non-empty length and SHA-256. The committed
integration fixtures force the selected protocol, issue `Range: bytes=0-`, and use one cache-busted
URL for both sides of each pair. The HTTP/2 fixture uses cleartext prior knowledge (h2c) so protocol
behavior can be verified without trusting a test certificate.

Run the focused scenario checks with:

```powershell
cargo test --locked -p download-manager-engine --test http1_lan_s163
cargo test --locked -p download-manager-engine --test http2_lan_s164
cargo test --locked -p download-manager-engine --test non_range_single_stream_s165
```

The fixture clients prove the local transport and reporting contracts only. Release-lab plans use
the same paired runner with the installed IDM and Download Manager wrapper commands, the controlled
LAN server URL, and exact version-command output captured in the report.

## Non-range single-stream parity

The non-range gate uses the same randomized runner with `scenario` set exactly to
`non-range-single-stream`, `protocol` set to `HTTP/1.1`, and at least 10 pairs. The controlled server
must return `200 OK` and must not receive a `Range` header from either wrapper. Both wrappers must
perform one request per pair member so segmentation cannot hide single-stream overhead.

For this scenario, `download-manager-benchmark` writes a specialized JSON and Markdown report that
embeds the complete paired evidence and reports median, p10, and p90 candidate completion-time
overhead relative to IDM. The gate passes when the median overhead is at most 3%, matching
`REQUIREMENTS.md` section 14.4. A failed gate is still written as valid evidence so S166 can preserve
and profile a measured regression.

The committed fixture uses the HTTP/1.1 client's `download-single-stream` mode and asserts all 20
requests omit `Range`. Its identical baseline/candidate programs validate transport, hashes,
cache-busting, threshold boundaries, and report reproducibility; only a release-lab run with the
installed IDM and product wrappers establishes actual parity.
