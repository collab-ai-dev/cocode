# coco-keyring-store

Cross-platform credential storage via the `keyring` crate (macOS Keychain, Linux Secret Service, Windows Credential Manager).

## Key Types

- `KeyringStore` trait — `load` / `save` / `delete` over `(service, account)`
- `DefaultKeyringStore` — `keyring::Entry` implementation
- `CredentialStoreError` — wraps `keyring::Error`
