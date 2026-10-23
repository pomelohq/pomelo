//! Project search for a remote app: the same walk and query as a local search, run on this machine, its files
//! pushed in batches as they turn up. A client's next search from the same view stops the one before.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use files::guard;
use files::search::{FileResult, SearchLine};
use pom_remote::server_api::{
    SearchDone, SearchFound, SearchStartParams, SEARCH_DONE_TOPIC, SEARCH_FOUND_TOPIC,
};
use pom_rpc::{error_code, Publisher, RemoteError};

/// Well below the frame limit, since JSON can escape some text to several times its size.
pub const PUSH_BYTES: usize = 256 * 1024;
/// A longer line is cut to this many bytes around its first match.
pub const LINE_BYTES: usize = 4096;
const FLUSH_EVERY: Duration = Duration::from_millis(100);

struct Running {
    channel: u32,
    searcher: String,
    cancel: Arc<AtomicBool>,
}

enum Message {
    File(FileResult),
    Done(Result<bool, String>),
}

#[derive(Default)]
pub struct SearchService {
    running: Arc<Mutex<HashMap<u64, Vec<Running>>>>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl SearchService {
    pub fn new() -> SearchService {
        SearchService::default()
    }

    /// Searches still running, for every connection.
    pub fn running(&self) -> usize {
        locked(&self.running).values().map(Vec::len).sum()
    }

    pub fn start(
        &self,
        connection: u64,
        root: &Path,
        params: SearchStartParams,
        publisher: Publisher,
    ) -> Result<(), RemoteError> {
        let invalid = |message: String| RemoteError::new(error_code::INVALID_PARAMS, message);
        params.search.query().map_err(invalid)?;
        params.search.scope().map_err(invalid)?;
        let root = guard::resolve(root, "").map_err(crate::fs::refused)?;
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let mut running = locked(&self.running);
            let mine = running.entry(connection).or_default();
            mine.retain(|search| {
                let replaced = search.searcher == params.searcher;
                if replaced {
                    search.cancel.store(true, Ordering::Relaxed);
                }
                !replaced
            });
            mine.push(Running {
                channel: params.channel,
                searcher: params.searcher.clone(),
                cancel: cancel.clone(),
            });
        }
        let (sender, receiver) = mpsc::channel();
        let request = params.search;
        let stop = cancel.clone();
        let walker = std::thread::Builder::new()
            .name("pom-server-search".into())
            .spawn(move || {
                let found = Mutex::new(sender.clone());
                let outcome = files::search::run(&root, &request, &stop, &|file| {
                    if locked(&found).send(Message::File(file)).is_err() {
                        stop.store(true, Ordering::Relaxed);
                    }
                });
                if sender.send(Message::Done(outcome)).is_err() {
                    stop.store(true, Ordering::Relaxed);
                }
            });
        if let Err(error) = walker {
            self.forget(connection, &cancel);
            return Err(RemoteError::new(error_code::INTERNAL, error.to_string()));
        }
        let running = self.running.clone();
        let pusher = std::thread::Builder::new()
            .name("pom-server-search-push".into())
            .spawn(move || {
                push(&receiver, &publisher, &cancel);
                let reason = if cancel.load(Ordering::Relaxed) {
                    "cancelled"
                } else {
                    "done"
                };
                publisher.finish(reason);
                forget(&running, connection, &cancel);
            });
        pusher
            .map(|_| ())
            .map_err(|error| RemoteError::new(error_code::INTERNAL, error.to_string()))
    }

    pub fn cancel(&self, connection: u64, channel: u32) {
        if let Some(mine) = locked(&self.running).get(&connection) {
            for search in mine.iter().filter(|search| search.channel == channel) {
                search.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn disconnect(&self, connection: u64) {
        if let Some(mine) = locked(&self.running).remove(&connection) {
            for search in mine {
                search.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    fn forget(&self, connection: u64, cancel: &Arc<AtomicBool>) {
        forget(&self.running, connection, cancel);
    }
}

fn forget(running: &Mutex<HashMap<u64, Vec<Running>>>, connection: u64, cancel: &Arc<AtomicBool>) {
    let mut running = locked(running);
    if let Some(mine) = running.get_mut(&connection) {
        mine.retain(|search| !Arc::ptr_eq(&search.cancel, cancel));
        if mine.is_empty() {
            running.remove(&connection);
        }
    }
}

/// Publishes what the walk finds until it ends, a push fails or the search is cancelled.
fn push(receiver: &mpsc::Receiver<Message>, publisher: &Publisher, cancel: &AtomicBool) {
    let mut batch = Batch::default();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let message = match receiver.recv_timeout(FLUSH_EVERY) {
            Ok(message) => message,
            Err(RecvTimeoutError::Timeout) => {
                if !batch.flush_if_due(publisher) {
                    cancel.store(true, Ordering::Relaxed);
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => Message::Done(Err("the search stopped".into())),
        };
        match message {
            Message::File(file) => {
                if !batch.add(file, publisher) {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            Message::Done(outcome) => {
                if !batch.flush(publisher) || cancel.load(Ordering::Relaxed) {
                    return;
                }
                let done = match outcome {
                    Ok(limited) => SearchDone {
                        limited,
                        error: None,
                    },
                    Err(error) => SearchDone {
                        limited: false,
                        error: Some(error),
                    },
                };
                if let Err(error) = publisher.publish(SEARCH_DONE_TOPIC, &done) {
                    eprintln!("search: {error}");
                }
                return;
            }
        }
    }
}

#[derive(Default)]
struct Batch {
    files: Vec<FileResult>,
    bytes: usize,
    since: Option<Instant>,
}

impl Batch {
    fn add(&mut self, file: FileResult, publisher: &Publisher) -> bool {
        let mut pieces = pieces(file, PUSH_BYTES);
        let last = pieces.pop();
        if !pieces.is_empty() && !self.flush(publisher) {
            return false;
        }
        for piece in pieces {
            let found = SearchFound {
                files: vec![piece],
                continued: true,
            };
            if publisher.publish(SEARCH_FOUND_TOPIC, &found).is_err() {
                return false;
            }
        }
        let Some(last) = last else {
            return true;
        };
        let size = file_bytes(&last);
        if self.bytes + size > PUSH_BYTES && !self.flush(publisher) {
            return false;
        }
        self.bytes += size;
        self.files.push(last);
        self.since.get_or_insert_with(Instant::now);
        self.flush_if_due(publisher)
    }

    fn flush_if_due(&mut self, publisher: &Publisher) -> bool {
        let due = self.bytes >= PUSH_BYTES
            || self
                .since
                .is_some_and(|since| since.elapsed() >= FLUSH_EVERY);
        !due || self.flush(publisher)
    }

    fn flush(&mut self, publisher: &Publisher) -> bool {
        self.since = None;
        self.bytes = 0;
        if self.files.is_empty() {
            return true;
        }
        let found = SearchFound {
            files: std::mem::take(&mut self.files),
            continued: false,
        };
        publisher.publish(SEARCH_FOUND_TOPIC, &found).is_ok()
    }
}

fn line_bytes(line: &SearchLine) -> usize {
    serde_json::to_vec(line).map_or(line.text.len() * 6, |json| json.len()) + 1
}

fn file_bytes(file: &FileResult) -> usize {
    file.path.len() * 2
        + 64
        + file
            .excerpts
            .iter()
            .flatten()
            .map(line_bytes)
            .sum::<usize>()
}

/// Cuts a very long line to [`LINE_BYTES`] around its first match, keeping the matches inside.
fn clip(line: &mut SearchLine) {
    if line.text.len() <= LINE_BYTES {
        return;
    }
    let first = line.hits.first().map_or(0, |hit| hit.start);
    let mut start = first.saturating_sub(LINE_BYTES / 4);
    while !line.text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (start + LINE_BYTES).min(line.text.len());
    while !line.text.is_char_boundary(end) {
        end -= 1;
    }
    line.hits = line
        .hits
        .iter()
        .filter(|hit| hit.start >= start && hit.start < end)
        .map(|hit| hit.start - start..hit.end.min(end) - start)
        .collect();
    line.text = line.text[start..end].to_string();
}

/// `file` in pieces of at most about `budget` bytes, split between lines; their match counts add up to its.
fn pieces(mut file: FileResult, budget: usize) -> Vec<FileResult> {
    for line in file.excerpts.iter_mut().flatten() {
        clip(line);
    }
    if file_bytes(&file) <= budget {
        return vec![file];
    }
    let header = file.path.len() * 2 + 64;
    let total = file.match_count;
    let mut pieces: Vec<FileResult> = Vec::new();
    let mut current = FileResult {
        path: file.path.clone(),
        ..FileResult::default()
    };
    let mut bytes = header;
    for excerpt in std::mem::take(&mut file.excerpts) {
        let mut open: Vec<SearchLine> = Vec::new();
        for line in excerpt {
            let size = line_bytes(&line);
            if bytes + size > budget && (bytes > header || !open.is_empty()) {
                if !open.is_empty() {
                    current.excerpts.push(std::mem::take(&mut open));
                }
                pieces.push(std::mem::replace(
                    &mut current,
                    FileResult {
                        path: file.path.clone(),
                        ..FileResult::default()
                    },
                ));
                bytes = header;
            }
            bytes += size;
            open.push(line);
        }
        if !open.is_empty() {
            current.excerpts.push(open);
        }
    }
    pieces.push(current);
    let mut counted = 0;
    let count = pieces.len();
    for (index, piece) in pieces.iter_mut().enumerate() {
        piece.match_count = if index + 1 == count {
            total.saturating_sub(counted)
        } else {
            piece
                .excerpts
                .iter()
                .flatten()
                .map(|line| line.hits.len())
                .sum()
        };
        counted += piece.match_count;
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(number: usize, text: &str) -> SearchLine {
        SearchLine {
            number,
            text: text.to_string(),
            hits: std::iter::once(0..1).collect(),
        }
    }

    fn tree(files: usize) -> std::io::Result<tempfile::TempDir> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir_all(temp.path().join("src"))?;
        for index in 0..files {
            let text = if index % 7 == 0 {
                "let needle = 1;\n"
            } else {
                "nothing\n"
            };
            std::fs::write(temp.path().join(format!("src/f{index}.rs")), text)?;
        }
        Ok(temp)
    }

    fn start_params(channel: u32, searcher: &str) -> SearchStartParams {
        SearchStartParams {
            channel,
            searcher: searcher.into(),
            search: files::search::SearchRequest {
                query: "needle".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Every push until the channel ends: the files found, the done value, and why it ended.
    fn collect(
        subscription: &pom_rpc::Subscription,
    ) -> (Vec<FileResult>, Option<SearchDone>, String) {
        let deadline = Instant::now() + Duration::from_secs(20);
        let (mut found, mut done) = (Vec::new(), None);
        while Instant::now() < deadline {
            match subscription.recv_timeout(Duration::from_millis(100)) {
                Ok(pom_rpc::PushEvent::Value { topic, value }) if topic == SEARCH_FOUND_TOPIC => {
                    let batch: SearchFound = serde_json::from_value(value).unwrap_or_default();
                    found.extend(batch.files);
                }
                Ok(pom_rpc::PushEvent::Value { topic, value }) if topic == SEARCH_DONE_TOPIC => {
                    done = serde_json::from_value(value).ok();
                }
                Ok(pom_rpc::PushEvent::End { reason }) => return (found, done, reason),
                _ => {}
            }
        }
        (found, done, "timed out".into())
    }

    #[test]
    fn a_search_pushes_what_a_local_one_finds_and_the_next_one_stops_it() -> std::io::Result<()> {
        let temp = tree(3000)?;
        let reject = |_: pom_rpc::Request, _: &pom_rpc::RequestContext| {
            Err(RemoteError::new(error_code::METHOD_NOT_FOUND, "none"))
        };
        let (client, server) = pom_rpc::mock::connected(pom_rpc::Config::default(), reject)
            .map_err(std::io::Error::other)?;
        let service = SearchService::new();
        let first = client.subscribe().map_err(std::io::Error::other)?;
        let second = client.subscribe().map_err(std::io::Error::other)?;
        for subscription in [&first, &second] {
            let publisher = server
                .publisher(subscription.channel())
                .map_err(std::io::Error::other)?;
            service
                .start(
                    1,
                    temp.path(),
                    start_params(subscription.channel(), "view-1"),
                    publisher,
                )
                .map_err(|error| std::io::Error::other(error.message))?;
        }
        let (_, done, reason) = collect(&first);
        assert_eq!(
            reason, "cancelled",
            "the next search from the view stops it"
        );
        assert!(done.is_none());

        let (mut found, done, reason) = collect(&second);
        assert_eq!(reason, "done");
        assert_eq!(done.map(|done| done.limited), Some(false));
        let local = Mutex::new(Vec::new());
        let request = start_params(0, "").search;
        files::search::run(temp.path(), &request, &AtomicBool::new(false), &|file| {
            locked(&local).push(file);
        })
        .map_err(std::io::Error::other)?;
        let mut local = local.into_inner().unwrap_or_default();
        local.sort_by(|a, b| a.path.cmp(&b.path));
        found.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(found.len(), 3000_usize.div_ceil(7));
        assert_eq!(found, local);
        let deadline = Instant::now() + Duration::from_secs(5);
        while service.running() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(service.running(), 0);

        let bad = client.subscribe().map_err(std::io::Error::other)?;
        let publisher = server
            .publisher(bad.channel())
            .map_err(std::io::Error::other)?;
        let mut params = start_params(bad.channel(), "view-1");
        params.search.options.regex = true;
        params.search.query = "(".into();
        assert!(service.start(1, temp.path(), params, publisher).is_err());
        Ok(())
    }

    #[test]
    fn a_large_file_splits_between_lines_and_keeps_its_count() {
        let lines: Vec<SearchLine> = (0..200).map(|n| line(n, &"x".repeat(100))).collect();
        let file = FileResult {
            path: "src/a.rs".into(),
            excerpts: vec![lines[..120].to_vec(), lines[120..].to_vec()],
            match_count: 200,
        };
        let split = pieces(file.clone(), 4096);
        assert!(split.len() > 1);
        assert!(split.iter().all(|piece| file_bytes(piece) <= 4096));
        assert_eq!(
            split.iter().map(|piece| piece.match_count).sum::<usize>(),
            200
        );
        let numbers: Vec<usize> = split
            .iter()
            .flat_map(|piece| piece.excerpts.iter().flatten().map(|line| line.number))
            .collect();
        assert_eq!(numbers, (0..200).collect::<Vec<_>>());
        assert_eq!(pieces(file.clone(), usize::MAX), vec![file]);
    }

    #[test]
    fn a_long_line_is_cut_around_its_first_match() {
        let text = format!("{}needle{}", "a".repeat(10_000), "\u{e9}".repeat(5_000));
        let mut long = SearchLine {
            number: 0,
            hits: std::iter::once(10_000..10_006).collect(),
            text,
        };
        clip(&mut long);
        assert!(long.text.len() <= LINE_BYTES);
        assert_eq!(long.hits.len(), 1);
        assert_eq!(&long.text[long.hits[0].clone()], "needle");
    }
}
