# PPK test fixtures

These private keys exist only to exercise the PPK v2/v3 parser. They do not
protect any account and must never be used outside the test suite.

- `ppk-v2-rsa.ppk`: unencrypted RSA, comment `imported-openssh-key`
- `ppk-v2-rsa-encrypted.ppk`: RSA, passphrase `v2-passphrase`
- `ppk-v3-ed25519.ppk`: unencrypted Ed25519
- `ppk-v3-ecdsa-encrypted.ppk`: NIST P-256, passphrase `test-passphrase`

The source keys were generated specifically for this repository with Windows
OpenSSH 8.1. The PPK blobs follow PuTTY's published format and use authenticated
private data. The encrypted v3 fixture deliberately uses small Argon2 settings
to keep the unit test fast; production keys should use PuTTY's stronger defaults.
