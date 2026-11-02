# Fuzzing

S159 uses stable-toolchain property fuzzing for the hostile native-message, HTTP header/range, and
filename/path boundaries. Every pass first replays committed minimized failures and then generates
new random cases. A failing case is shrunk and appended to
`crates/download-manager-native-host/tests/fuzz_s159.proptest-regressions` so it becomes a permanent
regression.

Run the required 20-minute smoke from the repository root:

```powershell
powershell -File scripts/test-fuzz-s159.ps1 -DurationSeconds 1200
```

For a quick local check while changing a target:

```powershell
powershell -File scripts/test-fuzz-s159.ps1 -DurationSeconds 10 -CasesPerPass 64
```

The runner returns nonzero on the first failure and prints the regression-corpus path. Commit any
new minimized regression before fixing the defect, then retain it after the fix so future runs replay
the exact input.
