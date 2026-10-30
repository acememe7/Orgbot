//! The one piece of state several bots touch at once.
//!
//! When bots take turns one at a time, the engine can simply be owned: whoever
//! is acting holds `&mut Engine` for the whole turn. Running turns
//! concurrently makes that impossible — a turn is mostly a model call, and
//! holding the engine across it would serialise the room again and defeat the
//! point.
//!
//! So the engine moves behind a lock, and the rule becomes: **hold it for a
//! command, never for a model call.** Rather than write that rule in a comment
//! and hope, this exposes the engine only through a closure. A `FnOnce(&mut
//! Engine)` cannot contain an `.await`, so the lock is structurally impossible
//! to hold across one. The rule is enforced by the shape of the API instead of
//! by everyone remembering it.
//!
//! What this does *not* do is stop two bots from wanting the same thing, and
//! it does not need to. Every command revalidates its preconditions at the
//! moment it applies: a claim goes through the task state machine, which
//! refuses a task that is already claimed, and anything touching work in
//! progress goes through `require_assignee`, which refuses a task held by
//! somebody else. A bot's view of the board is a suggestion made when its
//! prompt was built; the write is checked against the board as it is now. That
//! is what makes a stale read harmless rather than corrupting.

use std::cell::RefCell;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};

use orgbot_engine::Engine;

thread_local! {
    /// Engines this thread is already holding, by address.
    ///
    /// A plain mutex is not reentrant, so a method that holds the engine and
    /// then calls one that acquires it waits forever on itself. Nothing in the
    /// type system stops that — two shared borrows look perfectly reasonable —
    /// and the symptom is the worst one available: a room that simply stops,
    /// with no error, no log line, and nothing to search for.
    ///
    /// So the deadlock is turned into a panic, which is recoverable (see the
    /// note on poisoning) and, unlike a freeze, says what happened.
    static HELD: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// The room engine, shared by every bot taking a turn.
///
/// Cloning is cheap and gives another handle to the same engine.
#[derive(Clone)]
pub struct SharedEngine(Arc<Mutex<Engine>>);

impl SharedEngine {
    /// Put an engine behind the lock.
    pub fn new(engine: Engine) -> Self {
        Self(Arc::new(Mutex::new(engine)))
    }

    /// Do one thing with the engine, holding the lock only for that.
    ///
    /// The closure cannot await, which is the point: a model call must never
    /// happen with this held.
    pub fn with<T>(&self, act: impl FnOnce(&mut Engine) -> T) -> T {
        act(&mut self.lock())
    }

    /// Borrow the engine until the returned guard is dropped.
    ///
    /// Prefer [`Self::with`] where it fits. This exists for the many read-only
    /// call sites that were written against a plain `&Engine` and read one
    /// field — `engine().room().phase` and the like — where the guard is a
    /// temporary that drops at the end of the statement.
    ///
    /// Do not hold the result across an `.await`.
    pub fn borrow(&self) -> EngineRef<'_> {
        self.lock()
    }

    /// Borrow the engine mutably until the returned guard is dropped.
    ///
    /// The same caution applies, and more sharply: this blocks every bot.
    pub fn borrow_mut(&self) -> EngineRef<'_> {
        self.lock()
    }

    /// A poisoned lock is recovered rather than propagated.
    ///
    /// Poisoning means a bot panicked mid-command. Refusing to hand out the
    /// engine after that would take the whole room down — every other bot, and
    /// the operator's own commands — over one bad turn. The engine validates
    /// each command before it mutates, so what a panic can leave behind is a
    /// turn that did not finish, not a half-applied board.
    fn lock(&self) -> EngineRef<'_> {
        let address = Arc::as_ptr(&self.0) as usize;
        HELD.with(|held| {
            assert!(
                !held.borrow().contains(&address),
                "the room engine is already held on this thread; locking it \
                 again would hang the room with no error at all. Read what \
                 you need from the borrow you already have, or scope it so it \
                 drops first."
            );
            held.borrow_mut().push(address);
        });
        EngineRef {
            address,
            guard: self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }
}

/// The engine, borrowed.
///
/// Releases the lock when dropped. Do not hold one across an `.await`, and do
/// not ask for a second while holding one.
pub struct EngineRef<'a> {
    guard: MutexGuard<'a, Engine>,
    address: usize,
}

impl Deref for EngineRef<'_> {
    type Target = Engine;

    fn deref(&self) -> &Engine {
        &self.guard
    }
}

impl DerefMut for EngineRef<'_> {
    fn deref_mut(&mut self) -> &mut Engine {
        &mut self.guard
    }
}

impl Drop for EngineRef<'_> {
    fn drop(&mut self) {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(index) = held.iter().rposition(|held| *held == self.address) {
                held.remove(index);
            }
        });
    }
}

impl fmt::Debug for EngineRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.guard, f)
    }
}

impl fmt::Debug for SharedEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Locking to print would deadlock anything already holding it, which
        // is exactly when someone reaches for a debug print.
        f.write_str("SharedEngine(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orgbot_engine::Command;
    use orgbot_proto::room::{CompletionRule, RoomLimits, RoomSpec};
    use orgbot_proto::{AgentSpec, agent::ProviderRef};

    fn engine() -> Engine {
        Engine::new(RoomSpec {
            name: "shared".into(),
            goal: "prove the lock behaves".into(),
            roster: vec![],
            completion: CompletionRule::AllTasksTerminal,
            limits: RoomLimits::default(),
            access: Default::default(),
        })
    }

    #[test]
    fn a_handle_sees_what_another_handle_wrote() {
        let shared = SharedEngine::new(engine());
        let other = shared.clone();
        let spec = AgentSpec::new(
            "w1",
            "Worker",
            ProviderRef::Scripted {
                script: "s".into(),
            },
        );
        other.with(|engine| {
            engine
                .apply(Command::JoinAgent {
                    spec: Box::new(spec.clone()),
                })
                .unwrap()
        });
        assert!(
            shared.with(|engine| engine.agent_by_handle("w1").is_some()),
            "a clone is another handle to the same engine, not a copy of it"
        );
    }

    /// Locking twice on one thread must say so rather than hanging.
    ///
    /// This is not hypothetical: a predicate that held the engine and then
    /// called another that acquired it stopped a room dead, and the only
    /// evidence was a test that never finished.
    #[test]
    fn asking_for_the_engine_twice_on_one_thread_is_refused() {
        let shared = SharedEngine::new(engine());
        let held = shared.borrow();
        let again = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = shared.borrow();
        }));
        assert!(
            again.is_err(),
            "a nested borrow must panic; hanging is not an acceptable answer"
        );
        drop(held);
        // And the engine is usable again once the first borrow is done with.
        assert_eq!(shared.borrow().room().spec.name, "shared");
    }

    /// Two separate engines on one thread are not the same engine.
    #[test]
    fn holding_one_engine_does_not_block_another() {
        let first = SharedEngine::new(engine());
        let second = SharedEngine::new(engine());
        let held = first.borrow();
        assert_eq!(second.borrow().room().spec.name, "shared");
        drop(held);
    }

    /// A panic in one command must not cost the room every later one.
    #[test]
    fn a_poisoned_lock_still_hands_out_the_engine() {
        let shared = SharedEngine::new(engine());
        let panicking = shared.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            panicking.with(|_| panic!("a turn blew up"));
        }));
        assert!(result.is_err(), "the panic should have happened");
        assert_eq!(
            shared.with(|engine| engine.room().spec.name.clone()),
            "shared",
            "the room must still be reachable after a bot panicked"
        );
    }
}
