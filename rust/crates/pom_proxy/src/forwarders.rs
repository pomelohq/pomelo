//! Peers outside `proxy.trusted_proxies` that sent forwarded headers, kept in `proxy/forwarders.json` for
//! `pom doctor`: a reverse proxy left off the list has its headers ignored and everything it forwards needs the
//! token, which only shows up while requests flow.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

pub const FORWARDERS_FILE: &str = "proxy/forwarders.json";
const FORWARDERS_SCHEMA: &str = "pom.proxy_forwarders/v1";
const REWRITE_EVERY: Duration = Duration::from_secs(60 * 60);
const FORGET_AFTER_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const KEEP: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forwarder {
    pub address: String,
    pub last_seen_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ForwardersFile {
    schema: String,
    peers: Vec<Forwarder>,
}

pub fn read_forwarders(path: &Path) -> Vec<Forwarder> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<ForwardersFile>(&bytes).ok())
        .map(|file| file.peers)
        .unwrap_or_default()
}

/// The peers seen forwarding in the last week that the current settings still do not trust.
pub fn untrusted_forwarders(network: &pom_env::DevNetwork, state: &StateDir) -> Vec<String> {
    untrusted_since(
        read_forwarders(&state.path(FORWARDERS_FILE)),
        network,
        crate::faults::now_ms(),
    )
}

fn untrusted_since(
    peers: Vec<Forwarder>,
    network: &pom_env::DevNetwork,
    now_ms: u64,
) -> Vec<String> {
    peers
        .into_iter()
        .filter(|peer| now_ms.saturating_sub(peer.last_seen_ms) < FORGET_AFTER_MS)
        .filter(|peer| {
            peer.address
                .parse::<IpAddr>()
                .is_ok_and(|address| !network.trusts(address))
        })
        .map(|peer| peer.address)
        .collect()
}

pub(crate) struct Forwarders {
    path: Option<PathBuf>,
    noted: Mutex<HashMap<IpAddr, Instant>>,
}

impl Forwarders {
    pub(crate) fn new(path: Option<PathBuf>) -> Forwarders {
        Forwarders {
            path,
            noted: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn note(&self, address: IpAddr, now_ms: u64) {
        let Ok(mut noted) = self.noted.lock() else {
            return;
        };
        if noted
            .get(&address)
            .is_some_and(|at| at.elapsed() < REWRITE_EVERY)
        {
            return;
        }
        if noted.insert(address, Instant::now()).is_none() {
            eprintln!("dev proxy: {address} sends forwarded headers but is not in proxy.trusted_proxies, so they are ignored and its requests need the token (`pom proxy config set trusted_proxies {address}` if it is your reverse proxy)");
        }
        let Some(path) = &self.path else {
            return;
        };
        let mut peers = read_forwarders(path);
        let text = address.to_string();
        peers.retain(|peer| peer.address != text);
        peers.push(Forwarder {
            address: text,
            last_seen_ms: now_ms,
        });
        peers.sort_by_key(|peer| std::cmp::Reverse(peer.last_seen_ms));
        peers.truncate(KEEP);
        let file = ForwardersFile {
            schema: FORWARDERS_SCHEMA.into(),
            peers,
        };
        if let Err(error) = crate::activity::write_json(path, &file) {
            eprintln!("dev proxy: {}: {error}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(trusted: &[&str]) -> pom_env::DevNetwork {
        pom_env::DevNetwork::new(
            pom_env::DevPorts::default(),
            &pom_env::ProxySettings {
                trusted_proxies: trusted.iter().map(|range| range.to_string()).collect(),
                ..pom_env::ProxySettings::default()
            },
        )
    }

    #[test]
    fn a_forwarder_is_kept_until_trusted_or_a_week_old() {
        let temp = tempfile::tempdir().expect("temp");
        let path = temp.path().join(FORWARDERS_FILE);
        let forwarders = Forwarders::new(Some(path.clone()));
        let loopback: IpAddr = [127, 0, 0, 1].into();
        forwarders.note(loopback, 1_000);
        forwarders.note(loopback, 2_000);
        forwarders.note([10, 0, 0, 5].into(), 3_000);
        let peers = read_forwarders(&path);
        assert_eq!(peers.len(), 2, "{peers:?}");
        assert_eq!(peers[1].last_seen_ms, 1_000, "rewritten at most hourly");
        assert_eq!(
            untrusted_since(peers.clone(), &network(&[]), 4_000),
            ["10.0.0.5", "127.0.0.1"]
        );
        assert_eq!(
            untrusted_since(peers.clone(), &network(&["127.0.0.1"]), 4_000),
            ["10.0.0.5"]
        );
        assert!(untrusted_since(peers, &network(&[]), 3_000 + FORGET_AFTER_MS).is_empty());
    }
}
