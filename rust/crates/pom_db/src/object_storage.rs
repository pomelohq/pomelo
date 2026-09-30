//! S3-compatible object storage (the shared MinIO) over its HTTP API: buckets, one page of a prefix at a time,
//! reading, downloading and deleting objects, presigned links. Requests go through `curl`, and the signed
//! headers reach it on stdin so the keys never appear in a process listing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sigv4::{self, Credentials, EMPTY_PAYLOAD, UNSIGNED_PAYLOAD};

pub const PAGE_SIZE: usize = 50;
/// Folder totals come from at most this many objects, so a huge prefix still answers.
pub const STATS_CAP: u64 = 5_000;
const LIST_TIMEOUT: &str = "30";
const TRANSFER_TIMEOUT: &str = "600";
const CONNECT_TIMEOUT: &str = "5";
/// One curl run deletes at most this many objects.
const DELETE_BATCH: usize = 200;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// Stream the body to this file instead of answering it.
    pub output: Option<PathBuf>,
    /// Send this file as the body.
    pub input: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub trait HttpTransport: Send + Sync {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
    /// Sends each request (bodies discarded) and answers each status.
    fn send_all(&self, requests: &[HttpRequest]) -> Result<Vec<u16>, String> {
        requests
            .iter()
            .map(|request| self.send(request).map(|response| response.status))
            .collect()
    }
}

pub struct CurlTransport {
    pub program: PathBuf,
}

impl Default for CurlTransport {
    fn default() -> CurlTransport {
        CurlTransport {
            program: PathBuf::from("/usr/bin/curl"),
        }
    }
}

fn quoted(text: &str) -> String {
    format!(
        "\"{}\"",
        text.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "")
    )
}

fn curl_block(request: &HttpRequest) -> String {
    let mut config = format!(
        "url = {}\nrequest = {}\n",
        quoted(&request.url),
        quoted(&request.method)
    );
    for (name, value) in &request.headers {
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!("{name}: {value}"))
        ));
    }
    config
}

impl CurlTransport {
    fn run(&self, timeout: &str, extra: &[&str], config: &str) -> Result<Vec<u8>, String> {
        let mut child = Command::new(&self.program)
            .args([
                "-sS",
                "-g",
                "--connect-timeout",
                CONNECT_TIMEOUT,
                "--max-time",
                timeout,
            ])
            .args(extra)
            .args(["-K", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("curl: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| format!("curl: {error}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("curl: {error}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(output.stdout)
    }
}

impl HttpTransport for CurlTransport {
    fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let mut config = curl_block(request);
        let mut timeout = LIST_TIMEOUT;
        if let Some(path) = &request.output {
            config.push_str(&format!("output = {}\n", quoted(&path.to_string_lossy())));
            timeout = TRANSFER_TIMEOUT;
        }
        if let Some(path) = &request.input {
            config.push_str(&format!(
                "upload-file = {}\n",
                quoted(&path.to_string_lossy())
            ));
            timeout = TRANSFER_TIMEOUT;
        }
        let stdout = self.run(timeout, &["-D", "-"], &config)?;
        parse_response(&stdout)
    }

    fn send_all(&self, requests: &[HttpRequest]) -> Result<Vec<u16>, String> {
        let mut statuses = Vec::new();
        for chunk in requests.chunks(DELETE_BATCH) {
            let config: Vec<String> = chunk
                .iter()
                .map(|request| {
                    format!(
                        "{}output = \"/dev/null\"\nwrite-out = \"%{{http_code}}\\n\"\n",
                        curl_block(request)
                    )
                })
                .collect();
            let stdout = self.run(TRANSFER_TIMEOUT, &[], &config.join("next\n"))?;
            statuses.extend(
                String::from_utf8_lossy(&stdout)
                    .lines()
                    .filter_map(|line| line.trim().parse::<u16>().ok()),
            );
        }
        Ok(statuses)
    }
}

/// Headers (as `curl -D -` writes them, last block wins) then the body.
fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let mut rest = raw;
    let mut response = HttpResponse::default();
    while rest.starts_with(b"HTTP/") {
        let end = find(rest, b"\r\n\r\n")
            .map(|at| (at, 4))
            .or_else(|| find(rest, b"\n\n").map(|at| (at, 2)));
        let (head, body) = match end {
            Some((at, skip)) => (&rest[..at], &rest[at + skip..]),
            None => (rest, &rest[rest.len()..]),
        };
        let text = String::from_utf8_lossy(head);
        let mut lines = text.lines();
        response.status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        response.headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            .collect();
        rest = body;
    }
    if response.status == 0 {
        return Err("no answer from the object storage".into());
    }
    response.body = rest.to_vec();
    Ok(response)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectEntry {
    pub key: String,
    pub size: u64,
    pub modified: String,
    pub etag: String,
}

impl ObjectEntry {
    /// The last part of the key.
    pub fn name(&self) -> &str {
        self.key.rsplit('/').next().unwrap_or(&self.key)
    }
}

/// One page of a prefix: its sub-folders and objects, and where the next page starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub prefixes: Vec<String>,
    pub objects: Vec<ObjectEntry>,
    pub next: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrefixStats {
    pub objects: u64,
    pub bytes: u64,
    /// Counting stopped at `STATS_CAP`.
    pub capped: bool,
}

/// An object's first bytes and what the server says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectRead {
    pub content_type: String,
    pub bytes: Vec<u8>,
    /// The whole object's size, when the answer said.
    pub size: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectStore {
    /// `http://localhost:9000`
    pub base: String,
    /// `localhost:9000`
    pub host: String,
    pub credentials: Credentials,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn check(response: &HttpResponse, what: &str) -> Result<(), String> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let body = String::from_utf8_lossy(&response.body);
    let reason = tag_values(&body, "Message")
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("HTTP {}", response.status));
    Err(format!("{what}: {reason}"))
}

impl ObjectStore {
    pub fn new(host: &str, port: u16, credentials: Credentials) -> ObjectStore {
        let host = format!("{host}:{port}");
        ObjectStore {
            base: format!("http://{host}"),
            host,
            credentials,
        }
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
    ) -> HttpRequest {
        self.request_at(method, path, query, headers, EMPTY_PAYLOAD, now())
    }

    fn request_at(
        &self,
        method: &str,
        path: &str,
        query: &[(String, String)],
        headers: &[(String, String)],
        payload_hash: &str,
        unix_seconds: u64,
    ) -> HttpRequest {
        let signed = sigv4::sign(
            &sigv4::Request {
                method,
                host: &self.host,
                path,
                query,
                headers,
                payload_hash,
            },
            &self.credentials,
            unix_seconds,
        );
        let mut url = format!("{}{}", self.base, sigv4::uri_encode(path, true));
        if !query.is_empty() {
            let pairs: Vec<String> = query
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{}={}",
                        sigv4::uri_encode(key, false),
                        sigv4::uri_encode(value, false)
                    )
                })
                .collect();
            url.push('?');
            url.push_str(&pairs.join("&"));
        }
        HttpRequest {
            method: method.into(),
            url,
            headers: signed,
            output: None,
            input: None,
        }
    }

    fn object_path(bucket: &str, key: &str) -> String {
        format!("/{bucket}/{key}")
    }

    pub fn buckets(&self, transport: &dyn HttpTransport) -> Result<Vec<String>, String> {
        let response = transport.send(&self.request("GET", "/", &[], &[]))?;
        check(&response, "list buckets")?;
        let body = String::from_utf8_lossy(&response.body);
        Ok(tag_blocks(&body, "Bucket")
            .iter()
            .flat_map(|bucket| tag_values(bucket, "Name"))
            .collect())
    }

    /// One page (`limit` entries) of what sits directly under `prefix`, after `token`.
    pub fn list(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        prefix: &str,
        token: Option<&str>,
        limit: usize,
    ) -> Result<Listing, String> {
        let mut query = vec![
            ("delimiter".to_string(), "/".to_string()),
            ("list-type".to_string(), "2".to_string()),
            ("max-keys".to_string(), limit.to_string()),
            ("prefix".to_string(), prefix.to_string()),
        ];
        if let Some(token) = token {
            query.push(("continuation-token".into(), token.into()));
        }
        let response = transport.send(&self.request("GET", &format!("/{bucket}"), &query, &[]))?;
        check(&response, &format!("list {bucket}/{prefix}"))?;
        Ok(parse_listing(&String::from_utf8_lossy(&response.body)))
    }

    /// How many objects are under `prefix` (at any depth) and their total size.
    pub fn prefix_stats(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        prefix: &str,
    ) -> Result<PrefixStats, String> {
        let mut stats = PrefixStats::default();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![
                ("list-type".to_string(), "2".to_string()),
                ("max-keys".to_string(), "1000".to_string()),
                ("prefix".to_string(), prefix.to_string()),
            ];
            if let Some(token) = &token {
                query.push(("continuation-token".into(), token.clone()));
            }
            let response =
                transport.send(&self.request("GET", &format!("/{bucket}"), &query, &[]))?;
            check(&response, &format!("count {bucket}/{prefix}"))?;
            let listing = parse_listing(&String::from_utf8_lossy(&response.body));
            for object in &listing.objects {
                stats.objects += 1;
                stats.bytes += object.size;
            }
            if stats.objects >= STATS_CAP && listing.next.is_some() {
                stats.capped = true;
                return Ok(stats);
            }
            match listing.next {
                Some(next) => token = Some(next),
                None => return Ok(stats),
            }
        }
    }

    /// The object's first `limit` bytes (all of it when `None`).
    pub fn read(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        key: &str,
        limit: Option<u64>,
    ) -> Result<ObjectRead, String> {
        let headers: Vec<(String, String)> = limit
            .filter(|limit| *limit > 0)
            .map(|limit| vec![("Range".to_string(), format!("bytes=0-{}", limit - 1))])
            .unwrap_or_default();
        let response =
            transport.send(&self.request("GET", &Self::object_path(bucket, key), &[], &headers))?;
        if response.status == 416 {
            return Ok(ObjectRead {
                size: Some(0),
                ..ObjectRead::default()
            });
        }
        check(&response, &format!("read {key}"))?;
        let size = response
            .header("Content-Range")
            .and_then(|range| range.rsplit('/').next())
            .and_then(|total| total.parse().ok())
            .or_else(|| response.header("Content-Length")?.parse().ok());
        Ok(ObjectRead {
            content_type: response.header("Content-Type").unwrap_or_default().into(),
            size,
            bytes: response.body,
        })
    }

    /// Puts the file at `path` into the bucket as `key`; the body is streamed, so it goes unsigned.
    pub fn upload(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        key: &str,
        path: &Path,
    ) -> Result<(), String> {
        let mut request = self.request_at(
            "PUT",
            &Self::object_path(bucket, key),
            &[],
            &[],
            UNSIGNED_PAYLOAD,
            now(),
        );
        request.input = Some(path.to_path_buf());
        let response = transport.send(&request)?;
        check(&response, &format!("upload {key}"))
    }

    pub fn download(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        key: &str,
        path: &Path,
    ) -> Result<(), String> {
        let mut request = self.request("GET", &Self::object_path(bucket, key), &[], &[]);
        request.output = Some(path.to_path_buf());
        let response = transport.send(&request)?;
        if let Err(error) = check(&response, &format!("download {key}")) {
            if let Err(remove) = std::fs::remove_file(path) {
                eprintln!("object storage: remove {}: {remove}", path.display());
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn delete(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        key: &str,
    ) -> Result<(), String> {
        let response =
            transport.send(&self.request("DELETE", &Self::object_path(bucket, key), &[], &[]))?;
        check(&response, &format!("delete {key}"))
    }

    /// Deletes every object under `prefix`; answers how many went.
    pub fn delete_prefix(
        &self,
        transport: &dyn HttpTransport,
        bucket: &str,
        prefix: &str,
    ) -> Result<u64, String> {
        let mut deleted = 0;
        loop {
            let query = vec![
                ("list-type".to_string(), "2".to_string()),
                ("max-keys".to_string(), "1000".to_string()),
                ("prefix".to_string(), prefix.to_string()),
            ];
            let response =
                transport.send(&self.request("GET", &format!("/{bucket}"), &query, &[]))?;
            check(&response, &format!("list {bucket}/{prefix}"))?;
            let listing = parse_listing(&String::from_utf8_lossy(&response.body));
            if listing.objects.is_empty() {
                return Ok(deleted);
            }
            let requests: Vec<HttpRequest> = listing
                .objects
                .iter()
                .map(|object| {
                    self.request("DELETE", &Self::object_path(bucket, &object.key), &[], &[])
                })
                .collect();
            let statuses = transport.send_all(&requests)?;
            let gone = statuses
                .iter()
                .filter(|status| (200..300).contains(*status))
                .count();
            if gone == 0 {
                return Err(format!("delete {bucket}/{prefix}: the storage refused"));
            }
            deleted += gone as u64;
        }
    }

    pub fn presigned_url(&self, bucket: &str, key: &str, expires_seconds: u64) -> String {
        sigv4::presign(
            &self.base,
            &self.host,
            &Self::object_path(bucket, key),
            &self.credentials,
            now(),
            expires_seconds,
        )
    }
}

fn parse_listing(body: &str) -> Listing {
    Listing {
        prefixes: tag_blocks(body, "CommonPrefixes")
            .iter()
            .flat_map(|block| tag_values(block, "Prefix"))
            .collect(),
        objects: tag_blocks(body, "Contents")
            .iter()
            .map(|block| {
                let field = |name: &str| tag_values(block, name).into_iter().next();
                ObjectEntry {
                    key: field("Key").unwrap_or_default(),
                    size: field("Size")
                        .and_then(|size| size.parse().ok())
                        .unwrap_or(0),
                    modified: field("LastModified").unwrap_or_default(),
                    etag: field("ETag")
                        .unwrap_or_default()
                        .trim_matches('"')
                        .to_string(),
                }
            })
            .collect(),
        next: (tag_values(body, "IsTruncated").first().map(String::as_str) == Some("true"))
            .then(|| tag_values(body, "NextContinuationToken").into_iter().next())
            .flatten(),
    }
}

/// The raw inner text of each `<name>...</name>` in `body`.
fn tag_blocks<'a>(body: &'a str, name: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut blocks = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        blocks.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    blocks
}

fn tag_values(body: &str, name: &str) -> Vec<String> {
    tag_blocks(body, name)
        .into_iter()
        .map(unescape_xml)
        .collect()
}

fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#34;", "\"")
        .replace("&amp;", "&")
}

/// `1536` -> `1.5 KB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Answers queued responses in order and keeps what was asked.
    #[derive(Default)]
    pub(crate) struct FakeTransport {
        pub answers: Mutex<Vec<HttpResponse>>,
        pub asked: Mutex<Vec<HttpRequest>>,
    }

    impl FakeTransport {
        pub(crate) fn answering(bodies: &[&str]) -> FakeTransport {
            FakeTransport {
                answers: Mutex::new(
                    bodies
                        .iter()
                        .map(|body| HttpResponse {
                            status: 200,
                            headers: Vec::new(),
                            body: body.as_bytes().to_vec(),
                        })
                        .collect(),
                ),
                asked: Mutex::new(Vec::new()),
            }
        }

        fn urls(&self) -> Vec<String> {
            self.asked
                .lock()
                .map(|asked| asked.iter().map(|request| request.url.clone()).collect())
                .unwrap_or_default()
        }
    }

    impl HttpTransport for FakeTransport {
        fn send(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
            self.asked
                .lock()
                .map_err(|error| error.to_string())?
                .push(request.clone());
            let mut answers = self.answers.lock().map_err(|error| error.to_string())?;
            if answers.is_empty() {
                return Err("no more answers".into());
            }
            Ok(answers.remove(0))
        }
    }

    fn store() -> ObjectStore {
        ObjectStore::new(
            "localhost",
            9000,
            Credentials {
                access_key: "minioadmin".into(),
                secret_key: "minioadmin".into(),
                region: "us-east-1".into(),
            },
        )
    }

    const PAGE_ONE: &str = "<?xml version=\"1.0\"?><ListBucketResult><Name>files</Name><Prefix>uploads/</Prefix>\
        <IsTruncated>true</IsTruncated><NextContinuationToken>token+1/2</NextContinuationToken>\
        <Contents><Key>uploads/a &amp; b.png</Key><LastModified>2026-09-27T18:42:00.000Z</LastModified>\
        <ETag>&#34;9b2c&#34;</ETag><Size>1536</Size></Contents>\
        <CommonPrefixes><Prefix>uploads/avatars/</Prefix></CommonPrefixes></ListBucketResult>";
    const PAGE_TWO: &str = "<ListBucketResult><IsTruncated>false</IsTruncated>\
        <Contents><Key>uploads/z.csv</Key><Size>10</Size></Contents></ListBucketResult>";

    #[test]
    fn a_prefix_is_listed_a_page_at_a_time() -> Result<(), String> {
        let transport = FakeTransport::answering(&[PAGE_ONE, PAGE_TWO]);
        let store = store();
        let first = store.list(&transport, "files", "uploads/", None, PAGE_SIZE)?;
        assert_eq!(first.prefixes, ["uploads/avatars/"]);
        assert_eq!(first.objects.len(), 1);
        assert_eq!(first.objects[0].name(), "a & b.png");
        assert_eq!(first.objects[0].size, 1536);
        assert_eq!(first.objects[0].etag, "9b2c");
        assert_eq!(first.next.as_deref(), Some("token+1/2"));
        let second = store.list(&transport, "files", "uploads/", first.next.as_deref(), 50)?;
        assert_eq!(second.next, None);
        assert_eq!(second.objects[0].key, "uploads/z.csv");
        let urls = transport.urls();
        assert_eq!(
            urls[0],
            "http://localhost:9000/files?delimiter=%2F&list-type=2&max-keys=50&prefix=uploads%2F"
        );
        assert!(
            urls[1].ends_with("&continuation-token=token%2B1%2F2"),
            "{}",
            urls[1]
        );
        let asked = transport.asked.lock().map_err(|error| error.to_string())?;
        assert!(asked[0]
            .headers
            .iter()
            .any(|(name, value)| name == "authorization"
                && value.starts_with("AWS4-HMAC-SHA256 Credential=minioadmin/")));
        Ok(())
    }

    #[test]
    fn folder_totals_follow_every_page() -> Result<(), String> {
        let transport = FakeTransport::answering(&[PAGE_ONE, PAGE_TWO]);
        let stats = store().prefix_stats(&transport, "files", "uploads/")?;
        assert_eq!(
            stats,
            PrefixStats {
                objects: 2,
                bytes: 1546,
                capped: false
            }
        );
        Ok(())
    }

    #[test]
    fn buckets_and_errors_read_from_the_xml() -> Result<(), String> {
        let transport = FakeTransport::answering(&[
            "<ListAllMyBucketsResult><Buckets><Bucket><Name>files</Name></Bucket>\
             <Bucket><Name>exports</Name></Bucket></Buckets></ListAllMyBucketsResult>",
        ]);
        assert_eq!(store().buckets(&transport)?, ["files", "exports"]);
        let denied = FakeTransport {
            answers: Mutex::new(vec![HttpResponse {
                status: 403,
                headers: Vec::new(),
                body: b"<Error><Message>Access Denied.</Message></Error>".to_vec(),
            }]),
            asked: Mutex::new(Vec::new()),
        };
        assert_eq!(
            store().buckets(&denied),
            Err("list buckets: Access Denied.".into())
        );
        Ok(())
    }

    #[test]
    fn a_read_asks_for_the_first_bytes_and_learns_the_size() -> Result<(), String> {
        let transport = FakeTransport {
            answers: Mutex::new(vec![HttpResponse {
                status: 206,
                headers: vec![
                    ("Content-Type".into(), "text/csv".into()),
                    ("Content-Range".into(), "bytes 0-9/3400000".into()),
                ],
                body: b"id,email\n1".to_vec(),
            }]),
            asked: Mutex::new(Vec::new()),
        };
        let read = store().read(&transport, "files", "uploads/z.csv", Some(65_536))?;
        assert_eq!(read.content_type, "text/csv");
        assert_eq!(read.size, Some(3_400_000));
        let asked = transport.asked.lock().map_err(|error| error.to_string())?;
        assert!(asked[0]
            .headers
            .contains(&("range".to_string(), "bytes=0-65535".to_string())));
        assert_eq!(asked[0].url, "http://localhost:9000/files/uploads/z.csv");
        Ok(())
    }

    #[test]
    fn curl_output_splits_into_headers_and_body() -> Result<(), String> {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: image/png\r\n\r\n\x89PNG\r\n\r\nrest";
        let response = parse_response(raw)?;
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("image/png"));
        assert_eq!(response.body, b"\x89PNG\r\n\r\nrest");
        assert!(parse_response(b"").is_err());
        Ok(())
    }

    #[test]
    fn the_curl_config_quotes_what_it_passes() {
        let block = curl_block(&HttpRequest {
            method: "DELETE".into(),
            url: "http://localhost:9000/files/a\"b".into(),
            headers: vec![("authorization".into(), "AWS4 x\\y".into())],
            output: None,
            input: None,
        });
        assert_eq!(
            block,
            "url = \"http://localhost:9000/files/a\\\"b\"\nrequest = \"DELETE\"\nheader = \"authorization: AWS4 x\\\\y\"\n"
        );
    }

    #[test]
    fn sizes_read_like_a_file_manager() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1_932_735_283), "1.8 GB");
        assert_eq!(format_size(220 * 1024 * 1024), "220 MB");
    }
}
