use std::collections::BTreeMap;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use pom_paths::{write_atomic, StateDir};

const KEY_FILE: &str = "secret.key";
const KEY_LENGTH: usize = 32;
const NONCE_LENGTH: usize = 12;
/// Names starting with this are internal bookkeeping, hidden from listings.
const INTERNAL_PREFIX: &str = "__";

#[derive(Debug)]
pub enum SecretsError {
    Io(std::io::Error),
    Corrupt,
    /// Decryption failed: the machine key changed or the file was tampered with.
    Decrypt,
    Format(serde_json::Error),
}

impl std::fmt::Display for SecretsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretsError::Io(error) => write!(formatter, "secrets store: {error}"),
            SecretsError::Corrupt => formatter.write_str("secrets store corrupt"),
            SecretsError::Decrypt => formatter.write_str("secrets decrypt failed (key changed?)"),
            SecretsError::Format(error) => write!(formatter, "secrets store format: {error}"),
        }
    }
}

impl std::error::Error for SecretsError {}

impl From<std::io::Error> for SecretsError {
    fn from(error: std::io::Error) -> Self {
        SecretsError::Io(error)
    }
}

/// One session's secrets, encrypted at rest with a per-machine key:
/// `secrets/<session>.bin` = 12-byte nonce + AES-256-GCM(JSON object of name -> value).
/// Values only ever leave through `get`; agents and config see names.
pub struct SecretStore {
    state: StateDir,
    session: String,
}

impl SecretStore {
    pub fn new(state: StateDir, session: &str) -> Self {
        SecretStore {
            state,
            session: session.to_string(),
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<String>, SecretsError> {
        Ok(self.load()?.remove(name))
    }

    pub fn names(&self) -> Result<Vec<String>, SecretsError> {
        Ok(self
            .load()?
            .into_keys()
            .filter(|name| !name.starts_with(INTERNAL_PREFIX))
            .collect())
    }

    /// An empty value deletes the secret.
    pub fn set(&self, name: &str, value: &str) -> Result<(), SecretsError> {
        let mut secrets = self.load()?;
        if value.is_empty() {
            secrets.remove(name);
        } else {
            secrets.insert(name.to_string(), value.to_string());
        }
        self.save(&secrets)
    }

    /// Removes the whole store file (session deleted, or test cleanup).
    pub fn delete_all(&self) -> Result<(), SecretsError> {
        match std::fs::remove_file(self.store_path()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    pub fn store_path(&self) -> PathBuf {
        self.state
            .path("secrets")
            .join(format!("{}.bin", sanitize(&self.session)))
    }

    fn load(&self) -> Result<BTreeMap<String, String>, SecretsError> {
        let data = match std::fs::read(self.store_path()) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        if data.is_empty() {
            return Ok(BTreeMap::new());
        }
        if data.len() < NONCE_LENGTH {
            return Err(SecretsError::Corrupt);
        }
        let (nonce, sealed) = data.split_at(NONCE_LENGTH);
        let plain = self
            .cipher()?
            .decrypt(Nonce::from_slice(nonce), sealed)
            .map_err(|_| SecretsError::Decrypt)?;
        serde_json::from_slice(&plain).map_err(SecretsError::Format)
    }

    fn save(&self, secrets: &BTreeMap<String, String>) -> Result<(), SecretsError> {
        let plain = serde_json::to_vec(secrets).map_err(SecretsError::Format)?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let sealed = self
            .cipher()?
            .encrypt(&nonce, plain.as_slice())
            .map_err(|_| SecretsError::Corrupt)?;
        let mut data = nonce.to_vec();
        data.extend_from_slice(&sealed);
        write_atomic(&self.store_path(), &data, 0o600)?;
        Ok(())
    }

    /// The machine key is created on first use; losing it makes every store unreadable.
    fn cipher(&self) -> Result<Aes256Gcm, SecretsError> {
        let path = self.state.path(KEY_FILE);
        let key = match std::fs::read(&path) {
            Ok(key) if key.len() == KEY_LENGTH => key,
            _ => {
                let key = Aes256Gcm::generate_key(&mut OsRng).to_vec();
                write_atomic(&path, &key, 0o600)?;
                key
            }
        };
        Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key)))
    }
}

fn sanitize(session: &str) -> String {
    if session.is_empty() {
        return "_".to_string();
    }
    session
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '.' | ' ') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn store(session: &str) -> (tempfile::TempDir, SecretStore) {
        let temp = tempfile::tempdir().expect("temp dir");
        let store = SecretStore::new(StateDir::new(temp.path().join("pom")), session);
        (temp, store)
    }

    #[test]
    fn set_get_names_delete() -> Result<(), SecretsError> {
        let (_temp, store) = store("demo");
        assert_eq!(store.get("A")?, None);
        store.set("B", "2")?;
        store.set("A", "1")?;
        store.set("__meta", "x")?;
        assert_eq!(store.get("A")?.as_deref(), Some("1"));
        assert_eq!(store.names()?, ["A", "B"]);
        store.set("A", "")?;
        assert_eq!(store.names()?, ["B"]);
        store.delete_all()?;
        assert!(store.names()?.is_empty());
        Ok(())
    }

    #[test]
    fn files_are_private_and_encrypted() -> Result<(), SecretsError> {
        let (temp, store) = store("demo");
        store.set("TOKEN", "plain-value")?;
        let data = std::fs::read(store.store_path())?;
        assert!(!data.windows(11).any(|w| w == b"plain-value"));
        for path in [store.store_path(), temp.path().join("pom/secret.key")] {
            assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }

    #[test]
    fn changed_key_and_truncated_file_are_errors() -> Result<(), SecretsError> {
        let (temp, store) = store("demo");
        store.set("A", "1")?;
        std::fs::write(temp.path().join("pom/secret.key"), [7u8; 32])?;
        assert!(matches!(store.get("A"), Err(SecretsError::Decrypt)));
        std::fs::write(store.store_path(), [1u8; 5])?;
        assert!(matches!(store.get("A"), Err(SecretsError::Corrupt)));
        Ok(())
    }

    #[test]
    fn session_names_are_sanitized() {
        let (_temp, store) = store("my proj/v1.2");
        assert!(store.store_path().ends_with("secrets/my_proj_v1_2.bin"));
        assert_eq!(sanitize(""), "_");
    }
}
