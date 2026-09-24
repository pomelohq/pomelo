use std::path::Path;

use pom_config::yaml_node::{self, Node};

const CATALOG: &str = include_str!("../assets/catalog.yaml");
const PROXY_IMAGES: [&str; 5] = ["nginx", "traefik", "caddy", "haproxy", "envoy"];
/// Bare images that are backing services or dev tools, so an unknown bare image is not always the app.
const BARE_BACKING: [&str; 11] = [
    "adminer",
    "pgadmin",
    "phpmyadmin",
    "zookeeper",
    "consul",
    "vault",
    "etcd",
    "mosquitto",
    "eclipse-mosquitto",
    "prometheus",
    "grafana",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceKind {
    /// `build:` - the user's own code, run natively.
    App,
    /// A known backing image - a shared service.
    Shared,
    /// A reverse proxy the dev proxy replaces.
    Proxy,
    /// No image and no build.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub kind: ServiceKind,
    pub image: String,
    /// The catalog type of a shared service ("custom" for an unknown image).
    pub kind_name: String,
    pub strategy: String,
    /// How a branch is seeded: template | dump-restore | none.
    pub clone: String,
    pub ports: Vec<u16>,
    pub has_build: bool,
}

struct CatalogEntry {
    kind_name: String,
    match_image: Vec<String>,
    strategy: String,
    ports: Vec<u16>,
    clone: String,
}

fn catalog() -> Vec<CatalogEntry> {
    let Ok(Some(root)) = yaml_node::parse(CATALOG) else {
        return Vec::new();
    };
    root.items()
        .unwrap_or_default()
        .iter()
        .map(|entry| CatalogEntry {
            kind_name: entry
                .get("type")
                .map(Node::text)
                .unwrap_or_default()
                .to_string(),
            match_image: entry
                .get("match_image")
                .and_then(Node::items)
                .unwrap_or_default()
                .iter()
                .map(|item| item.text().to_lowercase())
                .collect(),
            strategy: entry
                .get("strategy")
                .map(Node::text)
                .unwrap_or_default()
                .to_string(),
            ports: entry
                .get("ports")
                .and_then(Node::items)
                .unwrap_or_default()
                .iter()
                .filter_map(|port| port.text().parse().ok())
                .collect(),
            clone: entry
                .get("clone")
                .map(Node::text)
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

fn classify(service: &mut ComposeService, catalog: &[CatalogEntry]) {
    if service.has_build {
        service.kind = ServiceKind::App;
        return;
    }
    if service.image.is_empty() {
        service.kind = ServiceKind::Unknown;
        return;
    }
    let image = service.image.to_lowercase();
    if PROXY_IMAGES.iter().any(|proxy| image.contains(proxy)) {
        service.kind = ServiceKind::Proxy;
        return;
    }
    if let Some(entry) = catalog.iter().find(|entry| {
        entry
            .match_image
            .iter()
            .any(|needle| image.contains(needle.as_str()))
    }) {
        service.kind = ServiceKind::Shared;
        service.kind_name = entry.kind_name.clone();
        service.strategy = entry.strategy.clone();
        service.clone = entry.clone.clone();
        service.ports = entry.ports.clone();
        return;
    }
    let name = image.split(':').next().unwrap_or_default();
    if BARE_BACKING.contains(&name) || name.contains('/') {
        service.kind = ServiceKind::Shared;
        service.kind_name = "custom".into();
    } else {
        // A bare unknown name (backend:latest) is nearly always an image built from the repo.
        service.kind = ServiceKind::App;
    }
}

fn read_compose(path: &Path) -> Option<Node> {
    yaml_node::parse(&std::fs::read_to_string(path).ok()?)
        .ok()
        .flatten()
}

fn service<'a>(document: &'a Node, name: &str) -> Option<&'a Node> {
    document.get("services")?.get(name)
}

/// The image a service runs, following `extends` (same file, then another file); `true` when it builds.
fn resolve_image(service_node: &Node, document: &Node, dir: &Path, depth: usize) -> (String, bool) {
    let image = service_node
        .get("image")
        .map(Node::text)
        .unwrap_or_default();
    if !image.is_empty() {
        return (image.to_string(), false);
    }
    if service_node.get("build").is_some() {
        return (String::new(), true);
    }
    let Some(extends) = service_node.get("extends").filter(|_| depth <= 6) else {
        return (String::new(), false);
    };
    let (base, file) = if extends.is_mapping() {
        (
            extends
                .get("service")
                .map(Node::text)
                .unwrap_or_default()
                .to_string(),
            extends
                .get("file")
                .map(Node::text)
                .unwrap_or_default()
                .to_string(),
        )
    } else {
        (extends.text().to_string(), String::new())
    };
    if base.is_empty() {
        return (String::new(), false);
    }
    if file.is_empty() {
        return match service(document, &base) {
            Some(parent) => resolve_image(parent, document, dir, depth + 1),
            None => (String::new(), false),
        };
    }
    let path = dir.join(&file);
    let Some(other) = read_compose(&path) else {
        return (String::new(), false);
    };
    match service(&other, &base) {
        Some(parent) => resolve_image(parent, &other, path.parent().unwrap_or(dir), depth + 1),
        None => (String::new(), false),
    }
}

/// The repo's docker-compose services, each classified; empty without a compose file.
pub fn parse_compose(repo: &Path) -> Vec<ComposeService> {
    let Some(path) = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ]
    .iter()
    .map(|name| repo.join(name))
    .find(|path| path.exists()) else {
        return Vec::new();
    };
    let Some(document) = read_compose(&path) else {
        return Vec::new();
    };
    let dir = path.parent().unwrap_or(repo);
    let catalog = catalog();
    let mut names: Vec<(String, Node)> = document
        .get("services")
        .and_then(Node::entries)
        .unwrap_or_default()
        .iter()
        .map(|(name, node)| (name.text().to_string(), node.clone()))
        .collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    names
        .into_iter()
        .map(|(name, node)| {
            let (image, has_build) = resolve_image(&node, &document, dir, 0);
            let mut service = ComposeService {
                name,
                kind: ServiceKind::Unknown,
                image,
                kind_name: String::new(),
                strategy: String::new(),
                clone: String::new(),
                ports: Vec::new(),
                has_build,
            };
            classify(&mut service, &catalog);
            service
        })
        .collect()
}
