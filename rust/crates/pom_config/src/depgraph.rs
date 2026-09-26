use std::collections::{HashMap, HashSet, VecDeque};

use crate::schema::Dir;

/// `depends_on` edges between the services of one repo.
pub struct DepGraph {
    dependencies: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleError;

impl std::fmt::Display for CycleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("dependency cycle detected among services")
    }
}

impl std::error::Error for CycleError {}

impl DepGraph {
    pub fn build(dir: &Dir) -> DepGraph {
        DepGraph {
            dependencies: dir
                .services
                .iter()
                .map(|(name, service)| (name.clone(), service.depends_on.clone()))
                .collect(),
        }
    }

    /// Requested services plus everything they transitively need, dependencies first. With no
    /// edges at all the request is returned untouched.
    pub fn start_order(&self, requested: &[String]) -> Result<Vec<String>, CycleError> {
        if self.dependencies.values().all(Vec::is_empty) {
            return Ok(requested.to_vec());
        }
        self.topological_sort(requested)
    }

    pub fn stop_order(&self, requested: &[String]) -> Result<Vec<String>, CycleError> {
        let mut order = self.start_order(requested)?;
        order.reverse();
        Ok(order)
    }

    fn deps_of(&self, name: &str) -> &[String] {
        self.dependencies.get(name).map_or(&[], Vec::as_slice)
    }

    // Kahn's algorithm; the queue is seeded in post-order so ties keep a stable, dependency-first order.
    fn topological_sort(&self, requested: &[String]) -> Result<Vec<String>, CycleError> {
        let all = self.collect_transitive(requested);
        let members: HashSet<&str> = all.iter().map(String::as_str).collect();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for node in &all {
            in_degree.entry(node).or_insert(0);
            for dependency in self.deps_of(node) {
                if members.contains(dependency.as_str()) {
                    dependents.entry(dependency).or_default().push(node);
                    *in_degree.entry(node).or_insert(0) += 1;
                }
            }
        }
        let mut queue: VecDeque<&str> = all
            .iter()
            .map(String::as_str)
            .filter(|node| in_degree.get(node) == Some(&0))
            .collect();
        let mut result = Vec::with_capacity(all.len());
        while let Some(node) = queue.pop_front() {
            result.push(node.to_string());
            for dependent in dependents.get(node).into_iter().flatten() {
                if let Some(degree) = in_degree.get_mut(dependent) {
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }
        if result.len() != all.len() {
            return Err(CycleError);
        }
        Ok(result)
    }

    fn collect_transitive(&self, requested: &[String]) -> Vec<String> {
        fn visit(graph: &DepGraph, name: &str, seen: &mut HashSet<String>, out: &mut Vec<String>) {
            if !seen.insert(name.to_string()) {
                return;
            }
            for dependency in graph.deps_of(name) {
                visit(graph, dependency, seen, out);
            }
            out.push(name.to_string());
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for name in requested {
            visit(self, name, &mut seen, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Service;

    fn dir(services: &[(&str, &[&str])]) -> Dir {
        let mut dir = Dir::default();
        for (name, deps) in services {
            dir.services.insert(
                name.to_string(),
                Service {
                    cmd: name.to_string(),
                    depends_on: deps.iter().map(|d| d.to_string()).collect(),
                    ..Service::default()
                },
            );
        }
        dir
    }

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    fn position(order: &[String], name: &str) -> usize {
        order.iter().position(|n| n == name).unwrap_or(usize::MAX)
    }

    #[test]
    fn no_deps_keeps_request() {
        let graph = DepGraph::build(&dir(&[("api", &[]), ("worker", &[])]));
        assert_eq!(
            graph.start_order(&names(&["worker", "api"])),
            Ok(names(&["worker", "api"]))
        );
    }

    #[test]
    fn deps_start_first() {
        let graph = DepGraph::build(&dir(&[
            ("api", &["worker"]),
            ("worker", &[]),
            ("scheduler", &["worker"]),
        ]));
        let order = graph
            .start_order(&names(&["api", "worker", "scheduler"]))
            .unwrap_or_default();
        assert!(position(&order, "worker") < position(&order, "api"));
        assert!(position(&order, "worker") < position(&order, "scheduler"));
    }

    #[test]
    fn stop_order_is_reversed() {
        let graph = DepGraph::build(&dir(&[("api", &["worker"]), ("worker", &[])]));
        let order = graph
            .stop_order(&names(&["api", "worker"]))
            .unwrap_or_default();
        assert!(position(&order, "api") < position(&order, "worker"));
    }

    #[test]
    fn cycle_is_an_error() {
        let graph = DepGraph::build(&dir(&[("a", &["b"]), ("b", &["a"])]));
        assert_eq!(graph.start_order(&names(&["a", "b"])), Err(CycleError));
    }

    #[test]
    fn transitive_deps_are_pulled_in() {
        let graph = DepGraph::build(&dir(&[
            ("api", &["worker"]),
            ("worker", &["db"]),
            ("db", &[]),
        ]));
        assert_eq!(
            graph.start_order(&names(&["api"])),
            Ok(names(&["db", "worker", "api"]))
        );
    }

    #[test]
    fn empty_request_is_empty() {
        let graph = DepGraph::build(&Dir::default());
        assert_eq!(graph.start_order(&[]), Ok(Vec::new()));
    }
}
