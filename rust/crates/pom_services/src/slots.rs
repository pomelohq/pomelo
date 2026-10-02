//! Shared services with a `capacity` serve several workspaces per container; each workspace gets a slot
//! (an index inside one instance, e.g. a Redis db number). Every project runs its own containers, so its
//! allocations live in `shared_slots.d/<session>.json`, guarded by `slots.lock`. A slot that was used, or is
//! about to be, waits in `pending` until the service's `slot_reset` has emptied it, so no workspace ever sees
//! another's data.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use pom_env::SlotAllocation;
use pom_paths::StateDir;
use serde::{Deserialize, Serialize};

/// The file every project shared before slots were kept per project; read once to adopt a project's own.
const LEGACY_FILE: &str = "shared_slots.json";
const PROJECT_DIR: &str = "shared_slots.d";
const LOCK_FILE: &str = "slots.lock";
/// A lock older than this belongs to a process that died holding it.
const STALE_LOCK: Duration = Duration::from_secs(10);
const LOCK_POLL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Slot {
    instance: u16,
    slot: u32,
}

/// A slot to empty before use: handed to `owner` (a fresh allocation), or given back (`owner` unset) and
/// blocked until its reset succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Pending {
    instance: u16,
    slot: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owner: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ServiceSlots {
    #[serde(default)]
    slots: BTreeMap<String, Slot>,
    #[serde(default)]
    instance_count: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<Pending>,
}

impl ServiceSlots {
    /// Drops trailing instances nobody uses; a re-created one starts with every slot pending again.
    fn trim(&mut self) {
        while self.instance_count > 1
            && !self
                .slots
                .values()
                .any(|slot| slot.instance == self.instance_count - 1)
        {
            self.instance_count -= 1;
        }
        let count = self.instance_count.max(1);
        self.pending.retain(|pending| pending.instance < count);
    }
}

/// A slot waiting for its reset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSlot {
    pub allocation: SlotAllocation,
    /// The workspace it was just handed to; `None` for a slot given back.
    pub owner: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SlotStore {
    state: StateDir,
    project: String,
}

impl SlotStore {
    pub fn new(state: StateDir, project: &str) -> SlotStore {
        SlotStore {
            state,
            project: project.to_string(),
        }
    }

    pub fn get(&self, service: &str, ws_key: &str) -> Option<SlotAllocation> {
        self.load()
            .get(service)
            .and_then(|slots| slots.slots.get(ws_key))
            .map(to_allocation)
    }

    /// How many containers the service runs to fit every workspace's slot.
    pub fn instance_count(&self, service: &str) -> u16 {
        self.load()
            .get(service)
            .map_or(1, |slots| slots.instance_count.max(1))
    }

    /// The workspaces holding a slot of the service.
    pub fn holders(&self, service: &str) -> Vec<String> {
        self.load()
            .get(service)
            .map(|slots| slots.slots.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The workspace's slot, reusing an existing one; otherwise the lowest free slot of the first
    /// instance with room, or slot 0 of a new instance. A fresh slot is pending until it is reset.
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
                instance_count: 1,
                ..ServiceSlots::default()
            });
        if let Some(existing) = slots.slots.get(ws_key) {
            return Ok(to_allocation(existing));
        }
        let capacity = u32::from(capacity);
        let blocked: Vec<Slot> = slots
            .pending
            .iter()
            .filter(|pending| pending.owner.is_none())
            .map(|pending| Slot {
                instance: pending.instance,
                slot: pending.slot,
            })
            .collect();
        let free = (0..slots.instance_count.max(1)).find_map(|instance| {
            let taken: Vec<u32> = slots
                .slots
                .values()
                .chain(blocked.iter())
                .filter(|slot| slot.instance == instance)
                .map(|slot| slot.slot)
                .collect();
            (0..capacity)
                .find(|index| !taken.contains(index))
                .map(|slot| Slot { instance, slot })
        });
        let slot = free.unwrap_or_else(|| {
            let instance = slots.instance_count.max(1);
            slots.instance_count = instance + 1;
            Slot { instance, slot: 0 }
        });
        slots.instance_count = slots.instance_count.max(1);
        slots.slots.insert(ws_key.to_string(), slot);
        slots.pending.push(Pending {
            instance: slot.instance,
            slot: slot.slot,
            owner: Some(ws_key.to_string()),
        });
        self.save(&all)?;
        Ok(to_allocation(&slot))
    }

    /// Takes the workspace's slot back; it stays out of use until its reset empties it.
    pub fn release(&self, service: &str, ws_key: &str) -> std::io::Result<()> {
        let _lock = self.lock()?;
        let mut all = self.load();
        let Some(slots) = all.get_mut(service) else {
            return Ok(());
        };
        let Some(slot) = slots.slots.remove(ws_key) else {
            return Ok(());
        };
        slots
            .pending
            .retain(|pending| (pending.instance, pending.slot) != (slot.instance, slot.slot));
        slots.pending.push(Pending {
            instance: slot.instance,
            slot: slot.slot,
            owner: None,
        });
        slots.trim();
        self.save(&all)
    }

    /// Every slot of the service waiting for its reset.
    pub fn pending(&self, service: &str) -> Vec<PendingSlot> {
        self.load()
            .get(service)
            .map(|slots| {
                slots
                    .pending
                    .iter()
                    .map(|pending| PendingSlot {
                        allocation: SlotAllocation {
                            instance: pending.instance,
                            slot: pending.slot,
                        },
                        owner: pending.owner.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The slot was emptied: a given-back one can be handed out again.
    pub fn cleared(&self, service: &str, allocation: SlotAllocation) -> std::io::Result<()> {
        let _lock = self.lock()?;
        let mut all = self.load();
        if let Some(slots) = all.get_mut(service) {
            slots.pending.retain(|pending| {
                (pending.instance, pending.slot) != (allocation.instance, allocation.slot)
            });
        }
        self.save(&all)
    }

    /// A fresh slot whose reset failed: block it and drop the workspace's claim, so it gets another.
    pub fn reject(&self, service: &str, ws_key: &str) -> std::io::Result<()> {
        let _lock = self.lock()?;
        let mut all = self.load();
        let Some(slots) = all.get_mut(service) else {
            return Ok(());
        };
        if let Some(slot) = slots.slots.remove(ws_key) {
            for pending in &mut slots.pending {
                if (pending.instance, pending.slot) == (slot.instance, slot.slot) {
                    pending.owner = None;
                }
            }
        }
        self.save(&all)
    }

    /// Once per project: takes over this project's workspaces from the file every project used to share,
    /// with their instance and slot numbers unchanged so no workspace moves to another database.
    pub fn adopt_legacy(&self, workspaces: &HashSet<String>) -> std::io::Result<()> {
        let _lock = self.lock()?;
        if self.path().exists() {
            return Ok(());
        }
        let legacy: BTreeMap<String, ServiceSlots> =
            std::fs::read_to_string(self.state.path(LEGACY_FILE))
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
        let mut adopted: BTreeMap<String, ServiceSlots> = BTreeMap::new();
        for (service, shared) in legacy {
            let slots: BTreeMap<String, Slot> = shared
                .slots
                .into_iter()
                .filter(|(ws_key, _)| workspaces.contains(ws_key))
                .collect();
            let instance_count = slots
                .values()
                .map(|slot| slot.instance + 1)
                .max()
                .unwrap_or(1);
            adopted.insert(
                service,
                ServiceSlots {
                    slots,
                    instance_count,
                    pending: Vec::new(),
                },
            );
        }
        self.save(&adopted)
    }

    fn path(&self) -> PathBuf {
        let name: String = self
            .project
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.state.path(format!("{PROJECT_DIR}/{name}.json"))
    }

    fn load(&self) -> BTreeMap<String, ServiceSlots> {
        std::fs::read_to_string(self.path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self, all: &BTreeMap<String, ServiceSlots>) -> std::io::Result<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_vec_pretty(all).map_err(std::io::Error::other)?;
        pom_paths::write_atomic(&path, &text, 0o644)
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
        let store = SlotStore::new(StateDir::new(temp.path()), "myproject");
        (temp, store)
    }

    fn clear_all(store: &SlotStore, service: &str) {
        for pending in store.pending(service) {
            store.cleared(service, pending.allocation).expect("cleared");
        }
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
        assert_eq!(
            store.pending("redis").len(),
            3,
            "every fresh slot waits for its reset"
        );
    }

    #[test]
    fn sixty_four_workspaces_fit_one_instance_and_the_sixty_fifth_opens_another() {
        let (_temp, store) = store();
        for index in 0..64 {
            let slot = store
                .allocate("redis", &format!("ws-{index}"), 64)
                .expect("slot");
            assert_eq!(slot.instance, 0, "workspace {index}");
        }
        let next = store.allocate("redis", "ws-64", 64).expect("slot");
        assert_eq!((next.instance, next.slot), (1, 0));
        assert_eq!(store.instance_count("redis"), 2);
    }

    #[test]
    fn a_given_back_slot_is_blocked_until_its_reset_and_empty_instances_go() {
        let (_temp, store) = store();
        for ws in ["ws-a", "ws-b", "ws-c"] {
            store.allocate("redis", ws, 2).expect("allocate");
        }
        clear_all(&store, "redis");
        store.release("redis", "ws-a").expect("release a");
        store.release("redis", "ws-c").expect("release c");
        assert_eq!(
            store.instance_count("redis"),
            1,
            "instance 1 has nobody left"
        );
        let blocked = store.allocate("redis", "ws-d", 2).expect("d");
        assert_eq!(
            (blocked.instance, blocked.slot),
            (1, 0),
            "slot 0 is not reset yet, so it is skipped"
        );
        store.release("redis", "ws-d").expect("release d");
        clear_all(&store, "redis");
        let reused = store.allocate("redis", "ws-e", 2).expect("e");
        assert_eq!(
            (reused.instance, reused.slot),
            (0, 0),
            "a reset slot is reused"
        );
        assert!(!store.state.path(LOCK_FILE).exists());
    }

    #[test]
    fn a_fresh_slot_whose_reset_failed_is_never_handed_out() {
        let (_temp, store) = store();
        let first = store.allocate("redis", "ws-a", 4).expect("a");
        store.reject("redis", "ws-a").expect("reject");
        let again = store.allocate("redis", "ws-a", 4).expect("a again");
        assert_ne!(again, first);
        assert_eq!(store.get("redis", "ws-a"), Some(again));
    }

    #[test]
    fn projects_keep_their_own_slots() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        let web = SlotStore::new(state.clone(), "myproject");
        let other = SlotStore::new(state, "otherproject");
        for index in 0..3 {
            web.allocate("redis", &format!("ws-w{index}"), 2)
                .expect("slot");
        }
        let main = other.allocate("redis", "ws-main", 2).expect("slot");
        assert_eq!(
            (main.instance, main.slot),
            (0, 0),
            "another project starts from zero"
        );
        assert_eq!(other.instance_count("redis"), 1);
        assert_eq!(web.instance_count("redis"), 2);
        let same_branch = web.allocate("redis", "ws-main", 2).expect("slot");
        assert_eq!(other.get("redis", "ws-main"), Some(main));
        assert_eq!(web.get("redis", "ws-main"), Some(same_branch));
    }

    #[test]
    fn the_shared_file_is_adopted_per_project_with_numbers_kept() {
        let temp = tempfile::tempdir().expect("temp");
        let state = StateDir::new(temp.path());
        std::fs::write(
            state.path(LEGACY_FILE),
            r#"{"redis":{"slots":{"ws-main":{"instance":0,"slot":4},"ws-feat-login":{"instance":1,"slot":3},"ws-gone":{"instance":0,"slot":9}},"instance_count":3}}"#,
        )
        .expect("write");
        let store = SlotStore::new(state.clone(), "myproject");
        let workspaces: HashSet<String> = ["ws-main", "ws-feat-login"]
            .iter()
            .map(|key| key.to_string())
            .collect();
        store.adopt_legacy(&workspaces).expect("adopt");
        assert_eq!(
            store.get("redis", "ws-feat-login"),
            Some(SlotAllocation {
                instance: 1,
                slot: 3
            })
        );
        assert_eq!(
            store.get("redis", "ws-main"),
            Some(SlotAllocation {
                instance: 0,
                slot: 4
            })
        );
        assert_eq!(
            store.get("redis", "ws-gone"),
            None,
            "another project's workspace stays out"
        );
        assert_eq!(
            store.instance_count("redis"),
            2,
            "counted from this project's own slots"
        );
        let other = SlotStore::new(state, "otherproject");
        other
            .adopt_legacy(&["ws-main".to_string()].into_iter().collect())
            .expect("adopt");
        assert_eq!(
            other.get("redis", "ws-main"),
            Some(SlotAllocation {
                instance: 0,
                slot: 4
            }),
            "a project with the same branch keeps its number too"
        );
        store
            .adopt_legacy(&HashSet::new())
            .expect("a second adopt is a no-op");
        assert!(store.get("redis", "ws-main").is_some());
    }
}
