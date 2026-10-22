//! GitHub's `X-Hub-Signature-256`: HMAC-SHA256 of the raw body under the webhook secret, as `sha256=<hex>`.

use sha2::{Digest, Sha256};

const BLOCK: usize = 64;
const PREFIX: &str = "sha256=";

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

/// The header value GitHub sends for `body` under `secret`.
pub fn sign(secret: &str, body: &[u8]) -> String {
    format!("{PREFIX}{}", hex(&hmac_sha256(secret.as_bytes(), body)))
}

/// Whether `header` is `body` signed with `secret`. Every byte is compared, so the time taken says nothing about
/// how much of a forged signature was right.
pub fn verify(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(given) = header
        .trim()
        .strip_prefix(PREFIX)
        .and_then(|digest| unhex(digest.trim()))
    else {
        return false;
    };
    let expected = hmac_sha256(secret.as_bytes(), body);
    given.len() == expected.len()
        && given
            .iter()
            .zip(expected)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_example_github_documents() {
        let header = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
        assert_eq!(sign("It's a Secret to Everybody", b"Hello, World!"), header);
        assert!(verify(
            "It's a Secret to Everybody",
            b"Hello, World!",
            header
        ));
    }

    #[test]
    fn anything_else_fails() {
        let secret = "s3cret";
        let good = sign(secret, b"{}");
        assert!(verify(secret, b"{}", &good));
        assert!(!verify(secret, b"{} ", &good));
        assert!(!verify("other", b"{}", &good));
        assert!(!verify(secret, b"{}", ""));
        assert!(!verify(secret, b"{}", good.trim_start_matches(PREFIX)));
        assert!(!verify(secret, b"{}", &good[..good.len() - 2]));
        assert!(!verify(secret, b"{}", "sha256=zz"));
        assert!(!verify(secret, b"{}", &format!("{good}00")));
    }
}
