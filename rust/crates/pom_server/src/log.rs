use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const ROTATE_AT: u64 = 5 * 1024 * 1024;

/// The server's own log: one line per event, rotated to `.log.1` past [`ROTATE_AT`]. Callers pass only
/// names, ids, sizes and timings, never request or answer contents.
pub struct ServerLog {
    path: PathBuf,
    rotate_at: u64,
    file: Mutex<Option<File>>,
}

fn open(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

fn timestamp() -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:03}", elapsed.as_secs(), elapsed.subsec_millis())
}

impl ServerLog {
    pub fn open(path: &Path, rotate_at: u64) -> io::Result<ServerLog> {
        Ok(ServerLog {
            path: path.to_path_buf(),
            rotate_at,
            file: Mutex::new(Some(open(path)?)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn file(&self) -> MutexGuard<'_, Option<File>> {
        self.file
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn rotate(&self, file: &mut Option<File>) -> io::Result<()> {
        let size = file
            .as_ref()
            .map(|file| file.metadata().map(|metadata| metadata.len()))
            .transpose()?
            .unwrap_or_default();
        if size < self.rotate_at {
            return Ok(());
        }
        *file = None;
        let mut rotated = self.path.clone().into_os_string();
        rotated.push(".1");
        std::fs::rename(&self.path, rotated)?;
        *file = Some(open(&self.path)?);
        Ok(())
    }

    /// Appends `ts <event>`; a failed write goes to stderr since there is nowhere else to say it.
    pub fn line(&self, event: &str) {
        let mut file = self.file();
        let written = self.rotate(&mut file).and_then(|()| match file.as_mut() {
            Some(file) => writeln!(file, "{} {event}", timestamp()),
            None => Ok(()),
        });
        if let Err(error) = written {
            eprintln!("pom server log {}: {error}", self.path.display());
        }
    }

    pub fn request(&self, record: &RequestRecord<'_>) {
        self.line(&format!(
            "conn={} method={} id={} in={} out={} ms={} result={}",
            record.connection,
            record.method,
            record.id,
            record.bytes_in,
            record.bytes_out,
            record.duration.as_millis(),
            record.result
        ));
    }
}

/// One served request, as the log keeps it.
pub struct RequestRecord<'a> {
    pub connection: u64,
    pub method: &'a str,
    pub id: u32,
    pub bytes_in: usize,
    pub bytes_out: usize,
    pub duration: Duration,
    pub result: &'a str,
}
