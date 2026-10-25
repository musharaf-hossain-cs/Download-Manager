# Requirement Traceability

This file defines the evidence format used as implementation proceeds. A requirement is not complete merely because it has a row; every `Verified` row must link to an automated test, review artifact, or reproducible command output.

## Status values

- `Unmapped`: no implementation slice owns the requirement yet.
- `Planned`: owning slice and intended evidence are known.
- `Implemented`: code exists but mandatory evidence is incomplete.
- `Verified`: all named evidence passes on the supported matrix.
- `Blocked`: a named, tracked condition prevents completion.

## Mapping template

| Requirement ID | Priority | Owning slice(s) | Status | Evidence | Notes |
|---|---|---|---|---|---|
| `EXAMPLE-001` | P0 | S000 | Planned | `path/to/test` or CI run URL | Replace this example when mappings begin. |

## Evidence rules

1. Use the exact stable ID from `REQUIREMENTS.md`.
2. Link the smallest automated test or artifact that proves the behavior.
3. Record each supported OS/browser variant when the requirement names a matrix.
4. A benchmark requirement links raw machine-readable results and the generated comparison report.
5. A security requirement links both the positive behavior and the rejection/abuse case.
6. Requirement changes follow section 20 of `REQUIREMENTS.md`; do not silently rewrite a mapping.

