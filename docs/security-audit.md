# Dependency security audit

This document describes the dependency state of the current tree. MeatShell now
uses `russh 0.62.6` and `russh-cryptovec 0.62.0`, which include the fixes for
RUSTSEC-2026-0153 and RUSTSEC-2026-0154. The exact versions remain authoritative
in `Cargo.lock`.

Install `cargo-audit` and scan the locked dependency graph with:

```powershell
cargo install cargo-audit --locked
cargo audit
```

The release workflow runs the same RustSec scan before its build jobs, and the
checked-in `.cargo/audit.toml` records the same three exceptions. Do not add a
new ignore without documenting the affected package, reachable input, expected
impact, compensating controls, and removal condition here.

## RUSTSEC-2023-0071

The `rsa` crate is susceptible to the Marvin timing attack during RSA private
key operations. RustSec currently lists no patched release. MeatShell retains
RSA key support for compatibility, so this is an accepted confidentiality risk
for users who authenticate with RSA private keys. Ed25519 and ECDSA keys avoid
this code path and are the recommended alternatives until a fixed `rsa` release
is available through the SSH dependency graph.

Remove this exception as soon as a patched compatible `rsa` release exists.

## RUSTSEC-2026-0194 and RUSTSEC-2026-0195

The two affected `quick-xml` versions are transitive Slint Linux dependencies.
One is used by Wayland protocol code generation and the other by
AccessKit/zbus bindings; MeatShell does not pass terminal, SSH, configuration,
or Debug API input to either XML parser. Their current parent dependency ranges
cannot select patched `quick-xml >= 0.41` releases.

These exceptions are constrained to that dependency graph. Remove them when a
Slint/accessibility dependency update accepts the patched parser, or reassess
immediately if MeatShell begins parsing user-controlled XML.

## Review gates

For a dependency or SSH authentication change:

1. Run `cargo audit` against the committed `Cargo.lock` and review every new
   advisory, including warnings hidden by transitive feature changes.
2. Run SSH authentication coverage for password, keyboard-interactive/MFA,
   private keys, encrypted keys, SFTP, proxy, and jump-host connections.
3. Confirm host-key verification and first-connect confirmation remain enabled.
4. Confirm rejected credentials use a fresh connection, prompt no more than
   three times, and stop immediately on cancellation.
5. Record any accepted advisory narrowly in both `.cargo/audit.toml` and this
   file, and keep the release workflow's comma-separated ignore list identical.

The loopback Debug API has a separate runtime boundary: it is opt-in, binds only
to `127.0.0.1`, authenticates every route with a constant-time Bearer-token
comparison, encrypts that token at rest, and applies body, input, screen, and
screenshot bounds. See [debug-api.md](debug-api.md) for its exposed data and
operational limits.
