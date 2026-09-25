//! Config bundles. A plain bundle is the merged `pom.yml` text. A sealed bundle also carries the session's
//! secrets: `POMBUNDLE1\n` + 16-byte salt + 12-byte nonce + AES-256-GCM of `{"config","secrets"}`, the key
//! derived from a password with scrypt (N=2^15, r=8, p=1), byte-compatible with bundles the previous core wrote.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use pom_paths::StateDir;
use pom_secrets::SecretStore;
use serde::{Deserialize, Serialize};

pub const MAGIC: &[u8] = b"POMBUNDLE1\n";
const SALT_LENGTH: usize = 16;
const NONCE_LENGTH: usize = 12;
const SCRYPT_LOG_N: u8 = 15;
const SCRYPT_R: u32 = 8;
const SCRYPT_P: u32 = 1;
pub const PLAIN_FILE_NAME: &str = "pom-config.yml";
pub const SEALED_FILE_NAME: &str = "pom-config.pombundle";
pub const ADAPT_SOURCE_FILE: &str = "pom-import-source.yml";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contents {
    pub config: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, String>,
}

pub fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(MAGIC)
}

pub fn seal(contents: &Contents, password: &str) -> Result<Vec<u8>, String> {
    if password.is_empty() {
        return Err("a password is required to include secrets".into());
    }
    let plain = serde_json::to_vec(contents).map_err(|error| error.to_string())?;
    let mut salt = [0u8; SALT_LENGTH];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; NONCE_LENGTH];
    OsRng.fill_bytes(&mut nonce);
    let sealed = cipher(password, &salt)?
        .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
        .map_err(|_| "encryption failed".to_string())?;
    let mut data = MAGIC.to_vec();
    data.extend_from_slice(&salt);
    data.extend_from_slice(&nonce);
    data.extend_from_slice(&sealed);
    Ok(data)
}

/// A plain bundle opens without a password; a sealed one needs the one it was sealed with.
pub fn open(data: &[u8], password: &str) -> Result<Contents, String> {
    let Some(body) = data.strip_prefix(MAGIC) else {
        return String::from_utf8(data.to_vec())
            .map(|config| Contents {
                config,
                secrets: BTreeMap::new(),
            })
            .map_err(|_| "the file is neither YAML text nor a config bundle".to_string());
    };
    if body.len() < SALT_LENGTH + NONCE_LENGTH {
        return Err("bundle truncated".into());
    }
    let (salt, rest) = body.split_at(SALT_LENGTH);
    let (nonce, sealed) = rest.split_at(NONCE_LENGTH);
    let plain = cipher(password, salt)?
        .decrypt(Nonce::from_slice(nonce), sealed)
        .map_err(|_| "wrong password or corrupt bundle".to_string())?;
    serde_json::from_slice(&plain).map_err(|error| format!("bundle contents: {error}"))
}

fn cipher(password: &str, salt: &[u8]) -> Result<Aes256Gcm, String> {
    let params = scrypt::Params::new(SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P, 32)
        .map_err(|error| error.to_string())?;
    let mut key = [0u8; 32];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut key)
        .map_err(|error| error.to_string())?;
    Aes256Gcm::new_from_slice(&key).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub file_name: &'static str,
    pub data: Vec<u8>,
}

/// The project's merged config; with a password, sealed together with every secret of the session.
pub fn export(
    config_path: &Path,
    state: &StateDir,
    session: &str,
    password: Option<&str>,
) -> Result<Export, String> {
    let config = pom_config::merged_yaml(config_path).map_err(|error| error.to_string())?;
    let Some(password) = password else {
        return Ok(Export {
            file_name: PLAIN_FILE_NAME,
            data: config.into_bytes(),
        });
    };
    let store = SecretStore::new(state.clone(), session);
    let mut secrets = BTreeMap::new();
    for name in store.names().map_err(|error| error.to_string())? {
        if let Some(value) = store.get(&name).map_err(|error| error.to_string())? {
            secrets.insert(name, value);
        }
    }
    Ok(Export {
        file_name: SEALED_FILE_NAME,
        data: seal(&Contents { config, secrets }, password)?,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    pub secrets_created: usize,
    pub split: bool,
}

/// Replaces `pom.yml` with `yaml` (the old one kept as `pom.yml.bak`, then tidied into `pom.d`) and/or stores
/// the bundle's secrets in the session.
pub fn apply(
    config_path: &Path,
    state: &StateDir,
    session: &str,
    yaml: Option<&str>,
    secrets: &BTreeMap<String, String>,
) -> Result<Applied, String> {
    let mut applied = Applied::default();
    if let Some(yaml) = yaml.filter(|yaml| !yaml.trim().is_empty()) {
        if let Ok(current) = std::fs::read(config_path) {
            let backup = config_path.with_extension("yml.bak");
            std::fs::write(&backup, current)
                .map_err(|error| format!("write {}: {error}", backup.display()))?;
        }
        std::fs::write(config_path, yaml)
            .map_err(|error| format!("write {}: {error}", config_path.display()))?;
        applied.split = pom_config::maintain::split(config_path, false).is_ok();
    }
    if !secrets.is_empty() {
        let store = SecretStore::new(state.clone(), session);
        for (name, value) in secrets {
            store
                .set(name, value)
                .map_err(|error| format!("secret {name}: {error}"))?;
            applied.secrets_created += 1;
        }
    }
    Ok(applied)
}

/// Leaves the imported config beside the project for an agent to merge; returns where it went.
pub fn stage_for_adapt(project_root: &Path, yaml: &str) -> Result<PathBuf, String> {
    let path = project_root.join(ADAPT_SOURCE_FILE);
    std::fs::write(&path, yaml).map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok(path)
}

/// The first turn for the agent that merges a staged import into the project's own config.
pub fn adapt_prompt() -> String {
    format!(
        "Merge {ADAPT_SOURCE_FILE} (in the project root) into this project's pom config. pom.yml only describes \
how to RUN the project, and templates use dot notation only. Steps: \
(1) Map the source's repos, paths, services and ports onto this project's repos. \
(2) Rewrite every legacy colon template to dot form ({{{{conn:x}}}} -> {{{{shared.x.url}}}}, {{{{host:x}}}} / \
{{{{port:x}}}} -> {{{{shared.x.host}}}} / {{{{shared.x.port}}}}, {{{{db:x}}}} -> {{{{db.x}}}}, {{{{user:x}}}} / \
{{{{pass:x}}}} / {{{{slot:x}}}} -> {{{{shared.x.*}}}}); never keep a colon form. \
(3) Drop what is not about running the project: proxy: and webhook: blocks (routing is automatic), app \
settings (ui, code_agents, jira), e2e, exposes: with {{{{var:}}}}, combinations and workspaces. \
(4) Replace any literal secret value with {{{{secret.NAME}}}}; the imported secrets are already stored. \
(5) Fold per-repo setup / migrate / seed / shortcuts into lifecycle.commands. \
Then run config_validate until it reports no errors, run config_normalize as the last step, and report \
what you merged, migrated and dropped. Delete {ADAPT_SOURCE_FILE} when done."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_bundles_open_with_their_password_only() {
        let contents = Contents {
            config: "session: shop\n".into(),
            secrets: BTreeMap::from([("API_KEY".to_string(), "k-123".to_string())]),
        };
        let data = seal(&contents, "hunter2").expect("seal");
        assert!(is_sealed(&data));
        assert_eq!(open(&data, "hunter2"), Ok(contents));
        assert_eq!(
            open(&data, "wrong"),
            Err("wrong password or corrupt bundle".to_string())
        );
        assert_eq!(
            open(&data[..MAGIC.len() + 4], "hunter2"),
            Err("bundle truncated".to_string())
        );
        assert!(seal(&Contents::default(), "").is_err());
    }

    #[test]
    fn plain_text_opens_as_config_without_secrets() {
        let opened = open(b"session: shop\n", "").expect("open");
        assert_eq!(opened.config, "session: shop\n");
        assert!(opened.secrets.is_empty());
    }

    #[test]
    fn prompt_names_the_staged_file_and_uses_dot_templates() {
        let prompt = adapt_prompt();
        assert!(prompt.contains("pom-import-source.yml"));
        assert!(prompt.contains("{{shared.x.url}}"));
        assert!(prompt.is_ascii());
    }
}
