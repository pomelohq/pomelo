//! `paste.put`: a file pasted in a terminal of the app (a screenshot), kept in a private folder on this machine
//! so the program in that terminal (an agent) can read it by its path. The client never names the path.

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use pom_remote::server_api::{PasteParams, Pasted, PASTE_LIMIT};
use pom_rpc::{error_code, Connection, RemoteError};

use crate::fs::{FsService, Upload};

/// How long a pasted file stays; an agent reads it when the prompt is sent, well before.
pub const PASTE_LIFETIME: Duration = Duration::from_secs(24 * 3600);
const MAX_EXTENSION: usize = 8;

pub struct PasteService {
    dir: PathBuf,
    lifetime: Duration,
}

impl PasteService {
    pub fn new(dir: PathBuf) -> PasteService {
        PasteService {
            dir,
            lifetime: PASTE_LIFETIME,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Receives the file over the client's stream, then writes it 0600 under a random name.
    pub fn put(
        &self,
        files: &FsService,
        connection: &Connection,
        params: &PasteParams,
    ) -> Result<Pasted, RemoteError> {
        let extension = check_extension(&params.extension)?;
        let upload = Upload {
            limit: PASTE_LIMIT,
            what: "a pasted file",
        };
        let bytes = files.receive_upload(connection, params.channel, params.size, upload)?;
        self.store(&bytes, &extension)
    }

    /// Writes `bytes` as a new file in the folder, after removing the files there past their lifetime.
    pub fn store(&self, bytes: &[u8], extension: &str) -> Result<Pasted, RemoteError> {
        let extension = check_extension(extension)?;
        pom_paths::ensure_private_dir(&self.dir, crate::peer::current_uid()).map_err(internal)?;
        self.remove_expired(SystemTime::now());
        let path = self
            .dir
            .join(format!("paste-{}.{extension}", random_name()?));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(internal)?;
        if let Err(error) = file.write_all(bytes) {
            if let Err(removing) = std::fs::remove_file(&path) {
                eprintln!("paste: {}: {removing}", path.display());
            }
            return Err(internal(error));
        }
        Ok(Pasted { path })
    }

    /// Removes the pasted files older than the lifetime; a file that cannot be read or removed is left.
    pub fn remove_expired(&self, now: SystemTime) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let expired = metadata.is_file()
                && metadata.modified().is_ok_and(|modified| {
                    now.duration_since(modified)
                        .is_ok_and(|age| age > self.lifetime)
                });
            if expired {
                if let Err(error) = std::fs::remove_file(entry.path()) {
                    eprintln!("paste: {}: {error}", entry.path().display());
                }
            }
        }
    }
}

fn internal(error: std::io::Error) -> RemoteError {
    RemoteError::new(crate::FAILED, error.to_string())
}

fn check_extension(extension: &str) -> Result<String, RemoteError> {
    let extension = extension.to_ascii_lowercase();
    let valid = !extension.is_empty()
        && extension.len() <= MAX_EXTENSION
        && extension.chars().all(|char| char.is_ascii_alphanumeric());
    if valid {
        Ok(extension)
    } else {
        Err(RemoteError::new(
            error_code::INVALID_PARAMS,
            "the extension must be a few letters or digits, such as png",
        ))
    }
}

fn random_name() -> Result<String, RemoteError> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(internal)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or_default()
    }

    #[test]
    fn a_pasted_file_lands_private_inside_the_folder_under_a_fresh_name() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let service = PasteService::new(temp.path().join("state/paste"));
        let first = service
            .store(b"\x89PNG one", "PNG")
            .map_err(|error| std::io::Error::other(error.message))?;
        let second = service
            .store(b"\x89PNG two", "png")
            .map_err(|error| std::io::Error::other(error.message))?;
        assert_ne!(first.path, second.path);
        for pasted in [&first, &second] {
            assert_eq!(pasted.path.parent(), Some(service.dir()));
            assert!(pasted.path.is_absolute());
            assert_eq!(
                pasted.path.extension().and_then(|ext| ext.to_str()),
                Some("png")
            );
            assert_eq!(mode(&pasted.path), 0o600);
        }
        assert_eq!(mode(service.dir()), 0o700);
        assert_eq!(std::fs::read(&first.path)?, b"\x89PNG one");
        Ok(())
    }

    #[test]
    fn an_extension_that_is_not_a_plain_word_is_refused() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let service = PasteService::new(temp.path().join("paste"));
        for extension in ["", "../png", "p/ng", "png.sh ", "averylongextension"] {
            let refused = service.store(b"x", extension);
            assert_eq!(
                refused.map_err(|error| error.code),
                Err(error_code::INVALID_PARAMS.to_string()),
                "{extension:?}"
            );
        }
        assert!(!service.dir().exists());
        Ok(())
    }

    #[test]
    fn files_past_their_lifetime_go_on_the_next_paste() -> std::io::Result<()> {
        let temp = tempfile::tempdir()?;
        let service = PasteService::new(temp.path().join("paste"));
        let old = service
            .store(b"old", "png")
            .map_err(|error| std::io::Error::other(error.message))?;
        std::fs::File::options()
            .write(true)
            .open(&old.path)?
            .set_modified(SystemTime::now() - PASTE_LIFETIME - Duration::from_secs(60))?;
        let fresh = service
            .store(b"new", "png")
            .map_err(|error| std::io::Error::other(error.message))?;
        assert!(!old.path.exists(), "a day-old paste is removed");
        assert!(fresh.path.exists());

        service.remove_expired(SystemTime::now() + PASTE_LIFETIME + Duration::from_secs(60));
        assert!(!fresh.path.exists());
        Ok(())
    }
}
