# Dependency security audit

GitHub Actions scans `Cargo.lock` with the RustSec advisory database on every
release-workflow run. To reproduce it locally:

```powershell
cargo install cargo-audit --locked
cargo audit `
  --ignore RUSTSEC-2023-0071 `
  --ignore RUSTSEC-2026-0194 `
  --ignore RUSTSEC-2026-0195
```

The exceptions are deliberately narrow:

- `RUSTSEC-2023-0071` affects RSA private-key operations and has no fixed
  release. The current russh release still depends on the affected RSA crate.
  MeatShell is an SSH client, so this path is reached only when a user selects
  an RSA private key for authentication. Prefer Ed25519 or ECDSA keys and keep
  host-key verification enabled. Remove this exception as soon as the upstream
  RSA implementation publishes a fixed release.
- `RUSTSEC-2026-0194` and `RUSTSEC-2026-0195` affect `quick-xml 0.39`. It is
  present only behind the Linux `wayland-scanner` procedural macro and parses
  protocol XML shipped by build dependencies. It is not linked into the
  MeatShell runtime and cannot parse terminal, SSH, SFTP, config, or user file
  input. The latest `wayland-scanner 0.31.10` still requires this version;
  remove both exceptions when it adopts `quick-xml >= 0.41`.

Warnings for unmaintained transitive crates are tracked separately from active
vulnerabilities. Dependabot checks Cargo and GitHub Actions weekly so patched
upstream releases are surfaced without silently changing a production build.
