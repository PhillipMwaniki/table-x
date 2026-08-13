//! OS keychain access.
//!
//! Passwords, key passphrases, and any other credential live here rather than in
//! the connection config, which is written to disk in plain JSON. On Windows this
//! is the Credential Manager, on macOS the Keychain, and on Linux the Secret
//! Service (GNOME Keyring, KWallet).

use tablex_core::error::{Error, Result};

/// Service name under which all entries are filed. Stable across versions —
/// changing it would orphan every stored credential.
const SERVICE: &str = "dev.tablex.app";

fn entry(key: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, key).map_err(|e| Error::Other(format!("keychain unavailable: {e}")))
}

/// Store or replace a secret.
pub fn set(key: &str, secret: &str) -> Result<()> {
    entry(key)?
        .set_password(secret)
        .map_err(|e| Error::Other(format!("could not save to keychain: {e}")))
}

/// Read a secret. A missing entry is `Ok(None)`, not an error: a connection that
/// has never been given a password is a normal state, not a failure.
pub fn get(key: &str) -> Result<Option<String>> {
    match entry(key)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(Error::Other(format!("could not read keychain: {e}"))),
    }
}

/// Remove a secret. Deleting something that is already absent succeeds, so that
/// deleting a connection is idempotent.
pub fn delete(key: &str) -> Result<()> {
    match entry(key)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(Error::Other(format!("could not delete from keychain: {e}"))),
    }
}
