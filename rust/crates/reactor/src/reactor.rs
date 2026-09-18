//! A small reactive app runtime in the reactive-entity style. State lives in `Entity<T>` handles; code mutates an entity
//! through a `Context<T>` (which also derefs to the whole `App`), and `cx.notify()` / `cx.emit(event)` wake any
//! `observe`/`subscribe` callbacks. Windows and views (rendering) build on top of this core; this module is
//! pure state/reactivity with no GPU or platform dependency, so it is unit-testable on its own.
//!
//! Storage note: entities are `Rc<RefCell<T>>` handles (idiomatic Rust) rather than the framework's slotmap+lease, but
//! the surface -- `App::new_entity`, `Entity::{read,update,downgrade}`, `Context::{notify,emit,observe,
//! subscribe}`, `Global` -- mirrors the framework so features port over directly.

use std::any::{Any, TypeId};
use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntityId(u64);

/// A strong handle to reactive state of type `T`. Cloning shares the same underlying state; dropping the last
/// handle drops the state.
pub struct Entity<T> {
    id: EntityId,
    cell: Rc<RefCell<T>>,
}

impl<T> Clone for Entity<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            cell: self.cell.clone(),
        }
    }
}

impl<T: 'static> Entity<T> {
    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn downgrade(&self) -> WeakEntity<T> {
        WeakEntity {
            id: self.id,
            weak: Rc::downgrade(&self.cell),
        }
    }

    /// Borrow the state immutably. `cx` proves you are inside the app.
    pub fn read<'a>(&'a self, _cx: &App) -> Ref<'a, T> {
        self.cell.borrow()
    }

    pub fn read_with<R>(&self, _cx: &App, f: impl FnOnce(&T) -> R) -> R {
        f(&self.cell.borrow())
    }

    /// Mutate the state with a `Context<T>`, then flush any notifications/events it queued.
    pub fn update<R>(&self, app: &mut App, f: impl FnOnce(&mut T, &mut Context<T>) -> R) -> R {
        let r = {
            let mut value = self.cell.borrow_mut();
            let mut cx = Context {
                app: &mut *app,
                this: self.downgrade(),
            };
            f(&mut value, &mut cx)
        };
        app.flush_effects();
        r
    }
}

/// A non-retaining handle; `upgrade` returns `None` once every `Entity<T>` is dropped.
pub struct WeakEntity<T> {
    id: EntityId,
    weak: Weak<RefCell<T>>,
}

impl<T> Clone for WeakEntity<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            weak: self.weak.clone(),
        }
    }
}

impl<T: 'static> WeakEntity<T> {
    pub fn id(&self) -> EntityId {
        self.id
    }

    pub fn upgrade(&self) -> Option<Entity<T>> {
        self.weak.upgrade().map(|cell| Entity { id: self.id, cell })
    }
}

/// A marker for singleton state read via `App::global`.
pub trait Global: 'static {}

/// Deregisters an `observe`/`subscribe` callback when dropped.
#[must_use = "a dropped Subscription stops observing immediately; keep it alive to keep observing"]
pub struct Subscription {
    active: Rc<Cell<bool>>,
}

impl Subscription {
    /// Keep the callback registered for the app's lifetime (the common case for long-lived views).
    pub fn detach(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.active.set(false);
    }
}

type NotifyCb = Rc<RefCell<dyn FnMut(&mut App)>>;
type EventCb = Rc<RefCell<dyn FnMut(&dyn Any, &mut App)>>;
type ObserverList = Vec<(Rc<Cell<bool>>, NotifyCb)>;
type ListenerList = Vec<(Rc<Cell<bool>>, EventCb)>;

/// The app context: owns globals and the observer/event wiring, drives the notify/flush cycle. Entities hold
/// their own state; `App` connects them.
#[derive(Default)]
pub struct App {
    next_id: u64,
    globals: HashMap<TypeId, Box<dyn Any>>,
    observers: HashMap<EntityId, ObserverList>,
    listeners: HashMap<(EntityId, TypeId), ListenerList>,
    pending_notify: Vec<EntityId>,
    pending_events: Vec<(EntityId, Box<dyn Any>)>,
    dirty: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new reactive entity. `build` gets a `Context` whose `WeakEntity` already resolves to the new
    /// entity (via `Rc::new_cyclic`), so it can `observe`/`subscribe` others and have those callbacks target
    /// this entity after construction.
    pub fn new_entity<T: 'static>(
        &mut self,
        build: impl FnOnce(&mut Context<T>) -> T,
    ) -> Entity<T> {
        let id = EntityId(self.next_id);
        self.next_id += 1;
        let cell = Rc::new_cyclic(|weak: &Weak<RefCell<T>>| {
            let mut cx = Context {
                app: &mut *self,
                this: WeakEntity {
                    id,
                    weak: weak.clone(),
                },
            };
            RefCell::new(build(&mut cx))
        });
        self.flush_effects();
        Entity { id, cell }
    }

    pub fn update_entity<T: 'static, R>(
        &mut self,
        entity: &Entity<T>,
        f: impl FnOnce(&mut T, &mut Context<T>) -> R,
    ) -> R {
        entity.update(self, f)
    }

    pub fn read_entity<T: 'static, R>(&self, entity: &Entity<T>, f: impl FnOnce(&T) -> R) -> R {
        entity.read_with(self, f)
    }

    pub fn set_global<G: Global>(&mut self, global: G) {
        self.globals.insert(TypeId::of::<G>(), Box::new(global));
    }

    pub fn try_global<G: Global>(&self) -> Option<&G> {
        self.globals
            .get(&TypeId::of::<G>())
            .and_then(|b| b.downcast_ref::<G>())
    }

    pub fn global<G: Global>(&self) -> &G {
        self.try_global::<G>().expect("global not set")
    }

    pub fn update_global<G: Global, R>(&mut self, f: impl FnOnce(&mut G, &mut App) -> R) -> R {
        let mut g = self
            .globals
            .remove(&TypeId::of::<G>())
            .expect("global not set");
        let r = f(g.downcast_mut::<G>().expect("global type"), self);
        self.globals.insert(TypeId::of::<G>(), g);
        self.flush_effects();
        r
    }

    /// Whether any entity notified since the last `take_dirty`. The host (window loop) redraws when true.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn flush_effects(&mut self) {
        // Notifications can cascade (an observer notifies another); bound the loop as a runaway backstop.
        for _ in 0..1000 {
            if self.pending_notify.is_empty() && self.pending_events.is_empty() {
                break;
            }
            let notify = std::mem::take(&mut self.pending_notify);
            let events = std::mem::take(&mut self.pending_events);

            for id in notify {
                self.dirty = true;
                let cbs: Vec<NotifyCb> = self
                    .observers
                    .get(&id)
                    .map(|v| {
                        v.iter()
                            .filter(|(a, _)| a.get())
                            .map(|(_, c)| c.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for cb in cbs {
                    (cb.borrow_mut())(self);
                }
                if let Some(v) = self.observers.get_mut(&id) {
                    v.retain(|(a, _)| a.get());
                }
            }
            for (emitter, event) in events {
                let key = (emitter, (*event).type_id());
                let cbs: Vec<EventCb> = self
                    .listeners
                    .get(&key)
                    .map(|v| {
                        v.iter()
                            .filter(|(a, _)| a.get())
                            .map(|(_, c)| c.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for cb in cbs {
                    (cb.borrow_mut())(event.as_ref(), self);
                }
                if let Some(v) = self.listeners.get_mut(&key) {
                    v.retain(|(a, _)| a.get());
                }
            }
        }
    }
}

/// The per-update context: derefs to `App` (so you can touch globals/other entities) while also carrying the
/// identity of the entity being updated for `notify`/`emit`/`observe`.
pub struct Context<'a, T> {
    app: &'a mut App,
    this: WeakEntity<T>,
}

impl<T> std::ops::Deref for Context<'_, T> {
    type Target = App;
    fn deref(&self) -> &App {
        self.app
    }
}

impl<T> std::ops::DerefMut for Context<'_, T> {
    fn deref_mut(&mut self) -> &mut App {
        self.app
    }
}

impl<'a, T: 'static> Context<'a, T> {
    pub fn entity_id(&self) -> EntityId {
        self.this.id()
    }

    pub fn weak(&self) -> WeakEntity<T> {
        self.this.clone()
    }

    pub fn new_entity<U: 'static>(
        &mut self,
        build: impl FnOnce(&mut Context<U>) -> U,
    ) -> Entity<U> {
        self.app.new_entity(build)
    }

    /// Mark this entity changed; observers fire and the app is flagged dirty (redraw).
    pub fn notify(&mut self) {
        self.app.pending_notify.push(self.this.id());
        self.app.dirty = true;
    }

    /// Emit an event to `subscribe`rs of this entity.
    pub fn emit<E: 'static>(&mut self, event: E) {
        self.app
            .pending_events
            .push((self.this.id(), Box::new(event)));
    }

    /// Run `on_notify` (against this entity) whenever `observed` notifies.
    pub fn observe<U: 'static>(
        &mut self,
        observed: &Entity<U>,
        mut on_notify: impl FnMut(&mut T, Entity<U>, &mut Context<T>) + 'static,
    ) -> Subscription {
        let this = self.this.clone();
        let observed_weak = observed.downgrade();
        let active = Rc::new(Cell::new(true));
        let cb_active = active.clone();
        let cb: NotifyCb = Rc::new(RefCell::new(move |app: &mut App| {
            if !cb_active.get() {
                return;
            }
            let (Some(obs), Some(obd)) = (this.upgrade(), observed_weak.upgrade()) else {
                cb_active.set(false);
                return;
            };
            obs.update(app, |t, cx| on_notify(t, obd.clone(), cx));
        }));
        self.app
            .observers
            .entry(observed.id())
            .or_default()
            .push((active.clone(), cb));
        Subscription { active }
    }

    /// Run `on_event` (against this entity) whenever `observed` emits an `E`.
    pub fn subscribe<U: 'static, E: 'static>(
        &mut self,
        observed: &Entity<U>,
        mut on_event: impl FnMut(&mut T, Entity<U>, &E, &mut Context<T>) + 'static,
    ) -> Subscription {
        let this = self.this.clone();
        let observed_weak = observed.downgrade();
        let active = Rc::new(Cell::new(true));
        let cb_active = active.clone();
        let cb: EventCb = Rc::new(RefCell::new(move |event: &dyn Any, app: &mut App| {
            if !cb_active.get() {
                return;
            }
            let (Some(obs), Some(obd)) = (this.upgrade(), observed_weak.upgrade()) else {
                cb_active.set(false);
                return;
            };
            if let Some(event) = event.downcast_ref::<E>() {
                obs.update(app, |t, cx| on_event(t, obd.clone(), event, cx));
            }
        }));
        self.app
            .listeners
            .entry((observed.id(), TypeId::of::<E>()))
            .or_default()
            .push((active.clone(), cb));
        Subscription { active }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Counter(i32);

    #[test]
    fn update_and_read() {
        let mut app = App::new();
        let c = app.new_entity(|_| Counter(0));
        c.update(&mut app, |c, cx| {
            c.0 += 5;
            cx.notify();
        });
        assert_eq!(app.read_entity(&c, |c| c.0), 5);
        assert!(app.take_dirty());
        assert!(!app.take_dirty());
    }

    #[test]
    fn observe_fires_on_notify() {
        let mut app = App::new();
        let source = app.new_entity(|_| Counter(0));
        let mirror = app.new_entity(|_| Counter(-1));
        {
            let source2 = source.clone();
            mirror.update(&mut app, |_, cx| {
                cx.observe(&source2, |m, src, cx| {
                    m.0 = src.read(cx).0 * 10;
                    cx.notify();
                })
                .detach();
            });
        }
        source.update(&mut app, |s, cx| {
            s.0 = 3;
            cx.notify();
        });
        assert_eq!(app.read_entity(&mirror, |m| m.0), 30);
    }

    #[derive(Clone, Copy)]
    struct Ping(i32);

    #[test]
    fn subscribe_receives_events() {
        let mut app = App::new();
        let source = app.new_entity(|_| Counter(0));
        let sink = app.new_entity(|_| Counter(0));
        {
            let source2 = source.clone();
            sink.update(&mut app, |_, cx| {
                cx.subscribe(&source2, |s, _src, ev: &Ping, _cx| {
                    s.0 += ev.0;
                })
                .detach();
            });
        }
        source.update(&mut app, |_, cx| cx.emit(Ping(7)));
        assert_eq!(app.read_entity(&sink, |s| s.0), 7);
    }

    struct Conf(String);
    impl Global for Conf {}

    #[test]
    fn globals() {
        let mut app = App::new();
        app.set_global(Conf("dark".into()));
        assert_eq!(app.global::<Conf>().0, "dark");
        app.update_global::<Conf, _>(|c, _| c.0 = "light".into());
        assert_eq!(app.global::<Conf>().0, "light");
    }

    #[test]
    fn dropped_subscription_stops() {
        let mut app = App::new();
        let source = app.new_entity(|_| Counter(0));
        let sink = app.new_entity(|_| Counter(0));
        let sub = {
            let source2 = source.clone();
            sink.update(&mut app, |_, cx| {
                cx.observe(&source2, |s, _src, cx| {
                    s.0 += 1;
                    cx.notify();
                })
            })
        };
        source.update(&mut app, |_, cx| cx.notify());
        assert_eq!(app.read_entity(&sink, |s| s.0), 1);
        drop(sub);
        source.update(&mut app, |_, cx| cx.notify());
        assert_eq!(app.read_entity(&sink, |s| s.0), 1);
    }
}
