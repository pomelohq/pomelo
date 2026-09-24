//! Shared services with a `capacity` serve several workspaces per container; each workspace gets a slot
//! (an index inside one instance, e.g. a Redis db number). Allocations live in the state dir's
//! `shared_slots.json`, guarded by `slots.lock`, in the same format the previous core wrote.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pom_env::SlotAllocation;
use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

const SLOTS_FILE: &str = "shared_slots.json";
const LOCK_FILE: &str = "slots.lock";
/// A lock older than this belongs to a process that died holding it.
const STALE_LOCK: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Slot {
    instance: u16,
    slot: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ServiceSlots {
    #[serde(default)]
    slots: BTreeMap<String, Slot>,
    #[serde(default)]
    instance_count: u16,
}

#[derive(Clone, Debug)]
pub struct SlotStore {
    state: StateDir,
}

impl SlotStore {
    pub fn new(state: StateDir) -> SlotStore {
        SlotStore { state }
    }

    pub fn get(&self, service: &str, ws_key: &str) -> Option<SlotAllocation> {
        self.load()
            .get(service)
            .and_then(|slots| slots.slots.get(ws_key))
            .map(to_allocation)
    }

    /// The workspace's slot, reusing an existing one; otherwise the lowest free slot of the first
    /// instance with room, or slot 0 of a new instance.
    pub fn allocate(
        &self,
        service: &str,
        ws_key: &str,
        capacity: u16,
    ) -> std::io::Result<SlotAllocation> {
        let _lock = self.lock()?;
        let mut all = self.load();
        let slots = all
            .entry(service.to_string())
            .or_insert_with(|| ServiceSlots {
                slots: BTreeMap::new(),
                instance_count: 1,
            });
        if let Some(existing) = slots.slots.get(ws_key) {
            return Ok(to_allocation(existing));
        }
        let capacity = u32::from(capacity);
        let free = (0..slots.instance_count.max(1)).find_map(|instance| {
            let used: Vec<u32> = slots
                .slots
                .values()
                .filter(|slot| slot.instance == instance)
                .map(|slot| slot.slot)
                .collect();
            let room = u32::try_from(used.len()).is_ok_and(|count| count < capacity);
            room.then(|| Slot {
                instance,
                slot: (0..capacity)
                    .find(|index| !used.contains(index))
                    .unwrap_or(0),
            })
        });
        let slot = free.unwrap_or_else(|| {
            let instance = slots.instance_count.max(1);
            slots.instance_count = instance + 1;
            Slot { instance, slot: 0 }
        });
        slots.instance_count = slots.instance_count.max(1);
        slots.slots.insert(ws_key.to_string(), slot);
        self.save(&all)?;
        Ok(to_allocation(&slot))
    }

    /// Frees the workspace's slot and drops trailing instances nobody uses any more.
    pub fn release(&self, service: &str, ws_key: &str) -> std::io::Result<()> {
        let _lock = self.lock()?;
        let mut all = self.load();
        let Some(slots) = all.get_mut(service) else {
            return Ok(());
        };
        slots.slots.remove(ws_key);
        while slots.instance_count > 1
            && !slots
                .slots
                .values()
                .any(|slot| slot.instance == slots.instance_count - 1)
        {
            slots.instance_count -= 1;
        }
        self.save(&all)
    }

    fn path(&self) -> PathBuf {
        self.state.path(SLOTS_FILE)
    }

    fn load(&self) -> BTreeMap<String, ServiceSlots> {
        std::fs::read_to_string(self.path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, all: &BTreeMap<String, ServiceSlots>) -> std::io::Result<()> {
        let text = serde_json::to_vec_pretty(all).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&self.path(), &text, 0o644)
    }

    fn lock(&self) -> std::io::Result<SlotLock> {
        let path = self.state.path(LOCK_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    write!(file, "{}", std::process::id())?;
                    return Ok(SlotLock { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age > STALE_LOCK);
                    if stale {
                        if let Err(error) = std::fs::remove_file(&path) {
                            if error.kind() != std::io::ErrorKind::NotFound {
                                return Err(error);
                            }
                        }
                        continue;
                    }
                    std::thread::sleep(LOCK_POLL);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

struct SlotLock {
    path: PathBuf,
}

impl Drop for SlotLock {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.path) {
            eprintln!("slots: unlock: {error}");
        }
    }
}

fn to_allocation(slot: &Slot) -> SlotAllocation {
    SlotAllocation {
        instance: slot.instance,
        slot: slot.slot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SlotStore) {
        let temp = tempfile::tempdir().expect("temp");
        let store = SlotStore::new(StateDir::new(temp.path()));
        (temp, store)
    }

    #[test]
    fn fills_an_instance_then_opens_the_next() {
        let (_temp, store) = store();
        let first = store.allocate("redis", "ws-a", 2).expect("a");
        let second = store.allocate("redis", "ws-b", 2).expect("b");
        let third = store.allocate("redis", "ws-c", 2).expect("c");
        assert_eq!((first.instance, first.slot), (0, 0));
        assert_eq!((second.instance, second.slot), (0, 1));
        assert_eq!((third.instance, third.slot), (1, 0));
        assert_eq!(store.allocate("redis", "ws-a", 2).expect("again"), first);
        assert_eq!(store.get("redis", "ws-c"), Some(third));
    }

    #[test]
    fn released_slots_are_reused_and_empty_instances_dropped() {
        let (_temp, store) = store();
        for ws in ["ws-a", "ws-b", "ws-c"] {
            store.allocate("redis", ws, 2).expect("allocate");
        }
        store.release("redis", "ws-a").expect("release a");
        store.release("redis", "ws-c").expect("release c");
        let reused = store.allocate("redis", "ws-d", 2).expect("d");
        assert_eq!((reused.instance, reused.slot), (0, 0));
        let text = std::fs::read_to_string(store.path()).expect("file");
        assert!(text.contains("\"instance_count\": 1"), "{text}");
        assert!(!store.state.path(LOCK_FILE).exists());
    }

    #[test]
    fn reads_the_previous_core_format() {
        let (_temp, store) = store();
        std::fs::write(
            store.path(),
            r#"{"redis":{"slots":{"ws-x":{"instance":1,"slot":3}},"instance_count":2}}"#,
        )
        .expect("write");
        assert_eq!(
            store.get("redis", "ws-x"),
            Some(SlotAllocation {
                instance: 1,
                slot: 3
            })
        );
    }
}
