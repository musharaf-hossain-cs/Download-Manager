# Windows compatibility lab

The release matrix runs the installed x64 package on dedicated, clean, self-hosted Windows
machines. The checked-in workflow never substitutes `windows-latest` for a missing operating
system or CPU architecture. A missing runner therefore leaves the compatibility requirement
unverified instead of producing a misleading pass.

Run the workflow in `.github/workflows/platform-compatibility.yml`. Each matrix member invokes:

```powershell
./scripts/test-platform-compatibility.ps1 -Profile <profile>
```

The script verifies the exact Windows build and OS architecture before building the explicit
`x86_64-pc-windows-msvc` target, installing it per user, launching the packaged CLI and native
host, checking all browser registrations, and exercising both uninstall choices. It always writes
bounded JSON evidence, including on failure.

The committed `docs/evidence/s191-windows-10-22h2-x64.json` artifact records a passing real-host
Windows 10 Pro 22H2/build-19045 installed-product smoke. It is evidence for that profile only and
does not substitute for any Windows 11 x64 or ARM64 matrix member.

The Windows 11 matrix is a 2026-11-03 support snapshot. It covers every Microsoft-serviced
general-availability x64 base build: Enterprise/Education 23H2 (build 22631), 24H2 (build 26100),
25H2 (build 26200), and 26H2 (build 26300). It separately covers the ARM-specific 26H1 build 28000
for the x64-emulation gate. Windows 10 22H2 remains an explicit application-support target even
where Microsoft OS servicing requires ESU or an LTSC edition. Refresh the matrix when Microsoft's
published servicing set changes; do not silently widen a build comparison. The authoritative sources are Microsoft's
[Windows 11 release information](https://learn.microsoft.com/windows/release-health/windows11-release-information)
and [Windows 11 Home and Pro lifecycle](https://learn.microsoft.com/lifecycle/products/windows-11-home-and-pro).

`-EnvironmentOnly` is a diagnostic that proves profile detection only. Its evidence status is
`environment-only`, never `passed`, and it cannot satisfy PLAT-01 or PLAT-02.
