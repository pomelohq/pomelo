//! AWS Signature Version 4 for S3-compatible object storage: signed request headers and presigned URLs.

use sha2::{Digest, Sha256};

pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
pub const EMPTY_PAYLOAD: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

/// One request to sign: `path` is the unencoded object path (`/bucket/key`), `query` unencoded pairs.
pub struct Request<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub path: &'a str,
    pub query: &'a [(String, String)],
    /// Headers besides `host`, `x-amz-date` and `x-amz-content-sha256`, which are added.
    pub headers: &'a [(String, String)],
    pub payload_hash: &'a str,
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|value| value ^ byte);
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(data)
        .finalize();
    Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize()
        .into()
}

/// Percent-encodes all but the unreserved characters; `/` stays when `keep_slash` (object paths).
pub fn uri_encode(text: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte))
            }
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `(20130524, 20130524T000000Z)` for a Unix time.
pub fn amz_dates(unix_seconds: u64) -> (String, String) {
    let days = (unix_seconds / 86_400) as i64;
    let seconds = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let date = format!("{year:04}{month:02}{day:02}");
    let time = format!(
        "{date}T{:02}{:02}{:02}Z",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    );
    (date, time)
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(key, value)| (uri_encode(key, false), uri_encode(value, false)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn signing_key(credentials: &Credentials, date: &str) -> [u8; 32] {
    let secret = format!("AWS4{}", credentials.secret_key);
    let key = hmac(secret.as_bytes(), date.as_bytes());
    let key = hmac(&key, credentials.region.as_bytes());
    let key = hmac(&key, SERVICE.as_bytes());
    hmac(&key, b"aws4_request")
}

fn scope(credentials: &Credentials, date: &str) -> String {
    format!("{date}/{}/{SERVICE}/aws4_request", credentials.region)
}

fn signature(credentials: &Credentials, date: &str, time: &str, canonical_request: &str) -> String {
    let to_sign = format!(
        "{ALGORITHM}\n{time}\n{}\n{}",
        scope(credentials, date),
        sha256_hex(canonical_request.as_bytes())
    );
    hex(&hmac(&signing_key(credentials, date), to_sign.as_bytes()))
}

/// The headers to send with the request, `Authorization` included.
pub fn sign(
    request: &Request<'_>,
    credentials: &Credentials,
    unix_seconds: u64,
) -> Vec<(String, String)> {
    let (date, time) = amz_dates(unix_seconds);
    let mut headers: Vec<(String, String)> = request
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    headers.push(("host".into(), request.host.to_string()));
    headers.push(("x-amz-content-sha256".into(), request.payload_hash.into()));
    headers.push(("x-amz-date".into(), time.clone()));
    headers.sort();
    let signed: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let signed = signed.join(";");
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    let canonical_request = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed}\n{}",
        request.method,
        uri_encode(request.path, true),
        canonical_query(request.query),
        request.payload_hash
    );
    let authorization = format!(
        "{ALGORITHM} Credential={}/{},SignedHeaders={signed},Signature={}",
        credentials.access_key,
        scope(credentials, &date),
        signature(credentials, &date, &time, &canonical_request)
    );
    headers.retain(|(name, _)| name != "host");
    headers.push(("authorization".into(), authorization));
    headers
}

/// A GET URL anyone can open until `expires_seconds` after `unix_seconds`.
pub fn presign(
    base: &str,
    host: &str,
    path: &str,
    credentials: &Credentials,
    unix_seconds: u64,
    expires_seconds: u64,
) -> String {
    let (date, time) = amz_dates(unix_seconds);
    let query = vec![
        ("X-Amz-Algorithm".to_string(), ALGORITHM.to_string()),
        (
            "X-Amz-Credential".to_string(),
            format!("{}/{}", credentials.access_key, scope(credentials, &date)),
        ),
        ("X-Amz-Date".to_string(), time.clone()),
        ("X-Amz-Expires".to_string(), expires_seconds.to_string()),
        ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
    ];
    let canonical_query = canonical_query(&query);
    let canonical_request = format!(
        "GET\n{}\n{canonical_query}\nhost:{host}\n\nhost\nUNSIGNED-PAYLOAD",
        uri_encode(path, true)
    );
    format!(
        "{base}{}?{canonical_query}&X-Amz-Signature={}",
        uri_encode(path, true),
        signature(credentials, &date, &time, &canonical_request)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Credentials {
        Credentials {
            access_key: "AKIAIOSFODNN7EXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".into(),
            region: "us-east-1".into(),
        }
    }

    const MAY_24_2013: u64 = 1_369_353_600;

    #[test]
    fn dates_are_utc_calendar_days() {
        assert_eq!(
            amz_dates(MAY_24_2013),
            ("20130524".into(), "20130524T000000Z".into())
        );
        assert_eq!(amz_dates(0).1, "19700101T000000Z");
        assert_eq!(amz_dates(951_782_400 + 3_723).1, "20000229T010203Z");
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        let key = [0x0bu8; 20];
        assert_eq!(
            hex(&hmac(&key, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn a_get_is_signed_like_the_published_example() {
        let headers = sign(
            &Request {
                method: "GET",
                host: "examplebucket.s3.amazonaws.com",
                path: "/test.txt",
                query: &[],
                headers: &[("Range".into(), "bytes=0-9".into())],
                payload_hash: EMPTY_PAYLOAD,
            },
            &example(),
            MAY_24_2013,
        );
        let authorization = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.as_str());
        assert_eq!(
            authorization,
            Some(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request,\
                 SignedHeaders=host;range;x-amz-content-sha256;x-amz-date,\
                 Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
            )
        );
        assert!(headers.iter().all(|(name, _)| name != "host"));
    }

    #[test]
    fn a_listing_is_signed_like_the_published_example() {
        let headers = sign(
            &Request {
                method: "GET",
                host: "examplebucket.s3.amazonaws.com",
                path: "/",
                query: &[
                    ("max-keys".into(), "2".into()),
                    ("prefix".into(), "J".into()),
                ],
                headers: &[],
                payload_hash: EMPTY_PAYLOAD,
            },
            &example(),
            MAY_24_2013,
        );
        assert!(headers.iter().any(|(name, value)| name == "authorization"
            && value.ends_with(
                "Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
            )));
    }

    #[test]
    fn a_presigned_url_matches_the_published_example() {
        let url = presign(
            "https://examplebucket.s3.amazonaws.com",
            "examplebucket.s3.amazonaws.com",
            "/test.txt",
            &example(),
            MAY_24_2013,
            86_400,
        );
        assert_eq!(
            url,
            "https://examplebucket.s3.amazonaws.com/test.txt?X-Amz-Algorithm=AWS4-HMAC-SHA256\
             &X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524%2Fus-east-1%2Fs3%2Faws4_request\
             &X-Amz-Date=20130524T000000Z&X-Amz-Expires=86400&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404"
        );
    }

    #[test]
    fn paths_keep_slashes_and_queries_do_not() {
        assert_eq!(uri_encode("/a b/c+d", true), "/a%20b/c%2Bd");
        assert_eq!(uri_encode("a/b", false), "a%2Fb");
    }
}
