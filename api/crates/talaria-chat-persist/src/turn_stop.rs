// A send's own name for its turn, so a stop can reach a turn the api has not
// heard of yet.
//
// The page names every turn it sends (`turnId` on /api/chat, a uuid it
// draws itself) and sends that name with its Stop. Two stops were lost
// without it:
//
// - A Stop that reached the api before the send had been read and its access
//   checked found nothing opening in the thread and answered 409, and the
//   turn ran on. The send's body is read in full before the route runs, so a
//   large paste, a slow uplink, or a starved database widens that window.
// - A Stop pressed on a new thread has no thread to name until the send's
//   response headers come back. The page kept that stop and sent it once the
//   headers named the thread, so a page closed meanwhile, or a view that
//   started a new chat, took the stop with it. The send itself runs on: the
//   app's proxy keeps a write going after its caller leaves.
//
// So a stop names the turn. A stop that finds no send for it waits, as the
// stop route waits for an opening turn to name itself, and then is held. The
// send, once it has checked its caller's access and claimed its thread's
// opening, says it is here (`turn_sent`): a waiting stop goes on to stop it
// the ordinary way, and a held one is handed to the opening, whose drain
// sends it the moment the turn names itself (`hold_stop`).
//
// WHOSE STOP. Every entry is keyed by the name and carries the user who made
// it. A stop applies only to a send by the same person, and to the thread
// it named when it named one. A name drawn by someone else, or a send for
// another thread, leaves the other's entry alone. So a stop never reaches
// another person's turn, whatever name it carries.
//
// BOUNDED. A held stop waits [`HELD_FOR`] for its send and is then dropped,
// a person holds at most [`HELD_PER_USER`] at once, and a turn that ended is
// remembered for as long, so a stop that comes after it gets the 409 at once
// instead of waiting for a send that already came. All of it is in memory,
// like the opening it hands off to: it means something only while this
// process holds the send.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::Instant;

/// How long a stop waits for its send, and how long an ended turn is
/// remembered. The patience Talaria gives an agent's response headers (the
/// gateway's headers window), and the page's own bound for a stop on a new
/// thread: a send slower than that to reach this process is not one a
/// person is still watching.
pub const HELD_FOR: Duration = Duration::from_secs(300);

/// The most stops one person can have waiting or held at once. A page holds
/// one per turn it streams; past this the oldest is let go.
pub const HELD_PER_USER: usize = 32;

static TURNS: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Default::default);

/// Woken whenever a send says it is here or its turn ends.
static TURN_SENT: LazyLock<Notify> = LazyLock::new(Notify::new);

struct Entry {
    user_id: String,
    state: State,
    /// Past this the entry means nothing and is swept.
    until: Instant,
    /// Insertion order, to let go of a person's oldest stop first.
    at: Instant,
}

#[derive(Clone)]
enum State {
    /// A stop for a send that has not come yet; the stop route is still
    /// waiting. `conversation_id` is the thread the stop named, if any.
    Waiting { conversation_id: Option<String> },
    /// The stop outlived its wait; the send hands it to its opening.
    Held { conversation_id: Option<String> },
    /// The send is here, in this thread.
    Sent { conversation_id: String },
    /// The send's turn is over, or the send queued behind another turn and
    /// has none of its own.
    Ended,
}

/// The name's key: its canonical text, so a stop and a send agree whatever
/// spelling of the uuid each was given.
fn key(turn_id: &str) -> String {
    talaria_params::uuid_canonical(turn_id).unwrap_or_else(|| turn_id.to_string())
}

fn thread_key(conversation_id: &str) -> String {
    talaria_params::uuid_canonical(conversation_id).unwrap_or_else(|| conversation_id.to_string())
}

fn sweep(turns: &mut HashMap<String, Entry>, now: Instant) {
    turns.retain(|_, e| e.until > now);
}

/// A send's hold on its name, from the moment it has checked its caller's
/// access and claimed its thread's opening. Dropped with the turn's opening
/// (when its drain stops reading, or when the send ends without one), which
/// marks the turn ended.
pub struct SentTurn {
    key: String,
    user_id: String,
    stop_held: bool,
}

impl SentTurn {
    /// A stop was held for this send before it came: the send hands it to
    /// its opening (`hold_stop`) for the drain to send.
    pub fn stop_held(&self) -> bool {
        self.stop_held
    }
}

impl Drop for SentTurn {
    fn drop(&mut self) {
        if let Ok(mut turns) = TURNS.lock()
            && let Some(e) = turns.get_mut(&self.key)
            && e.user_id == self.user_id
            && matches!(e.state, State::Sent { .. })
        {
            e.state = State::Ended;
            e.until = Instant::now() + HELD_FOR;
        }
        TURN_SENT.notify_waiters();
    }
}

/// The send named `turn_id` is here, for `conversation_id`, on behalf of
/// `user_id`. Call it after the caller's access is checked and the thread's
/// opening claimed, so a stop that wakes on it finds the opening.
pub fn turn_sent(turn_id: &str, user_id: &str, conversation_id: &str) -> SentTurn {
    let key = key(turn_id);
    let thread = thread_key(conversation_id);
    let now = Instant::now();
    let mut stop_held = false;
    if let Ok(mut turns) = TURNS.lock() {
        sweep(&mut turns, now);
        // A stop applies to this send only if it is the same person's, and
        // for this thread when it named one. Anything else under this name
        // is not this send's, and is replaced.
        let ours = |e: &Entry, wanted: &Option<String>| {
            e.user_id == user_id && wanted.as_deref().is_none_or(|c| thread_key(c) == thread)
        };
        if let Some(e) = turns.get(&key) {
            stop_held =
                matches!(&e.state, State::Held { conversation_id } if ours(e, conversation_id));
        }
        turns.insert(
            key.clone(),
            Entry {
                user_id: user_id.to_string(),
                state: State::Sent {
                    conversation_id: thread.clone(),
                },
                until: now + HELD_FOR,
                at: now,
            },
        );
    }
    TURN_SENT.notify_waiters();
    SentTurn {
        key,
        user_id: user_id.to_string(),
        stop_held,
    }
}

/// What a stop that names a turn found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnAwaited {
    /// The send is here, in this thread, with its opening claimed.
    Sent { conversation_id: String },
    /// Nothing of this name's to stop: its turn ended, it queued behind
    /// another turn, or the name is another person's or another thread's.
    Nothing,
    /// The send had not come by the deadline: the stop is held, and the send
    /// hands it to its opening when it comes.
    Held,
}

/// Wait, until `deadline`, for the send named `turn_id` to say it is here.
/// `conversation_id` is the thread the stop named (None for a new thread,
/// whose id the page does not have yet).
///
/// A stop that leaves the wait without its send, at the deadline or because
/// its own request went away, is held: the person asked for it, and the send
/// hands it to its opening when it comes.
pub async fn await_sent_turn(
    turn_id: &str,
    user_id: &str,
    conversation_id: Option<&str>,
    deadline: Instant,
) -> TurnAwaited {
    let key = key(turn_id);
    let wanted = conversation_id.map(thread_key);
    // The stop's own entry, if nothing is under this name yet.
    if let Ok(mut turns) = TURNS.lock() {
        let now = Instant::now();
        sweep(&mut turns, now);
        if !turns.contains_key(&key) {
            // Let go of this person's oldest stops past the bound.
            let mut mine: Vec<(Instant, String)> = turns
                .iter()
                .filter(|(_, e)| {
                    e.user_id == user_id
                        && matches!(e.state, State::Waiting { .. } | State::Held { .. })
                })
                .map(|(k, e)| (e.at, k.clone()))
                .collect();
            if mine.len() >= HELD_PER_USER {
                mine.sort();
                for (_, k) in mine.iter().take(mine.len() + 1 - HELD_PER_USER) {
                    turns.remove(k);
                }
            }
            turns.insert(
                key.clone(),
                Entry {
                    user_id: user_id.to_string(),
                    state: State::Waiting {
                        conversation_id: wanted.clone(),
                    },
                    until: now + HELD_FOR,
                    at: now,
                },
            );
        }
    }
    let leaving = HoldOnLeave {
        key: key.clone(),
        user_id: user_id.to_string(),
    };
    loop {
        // Registered before the read, so a send between the read and the
        // wait still wakes it.
        let changed = TURN_SENT.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if let Some(answer) = found(&key, user_id, wanted.as_deref()) {
            return answer;
        }
        tokio::select! {
            () = &mut changed => {}
            () = tokio::time::sleep_until(deadline) => break,
        }
    }
    drop(leaving);
    found(&key, user_id, wanted.as_deref()).unwrap_or(TurnAwaited::Held)
}

/// What the entry under `key` says to a stop by `user_id`, or None while it
/// is still waiting for its send.
fn found(key: &str, user_id: &str, wanted: Option<&str>) -> Option<TurnAwaited> {
    let turns = TURNS.lock().ok()?;
    let Some(e) = turns.get(key) else {
        // Swept, or let go of past the bound: nothing to wait for.
        return Some(TurnAwaited::Nothing);
    };
    if e.user_id != user_id {
        return Some(TurnAwaited::Nothing);
    }
    match &e.state {
        State::Sent { conversation_id } => Some(if wanted.is_none_or(|w| w == conversation_id) {
            TurnAwaited::Sent {
                conversation_id: conversation_id.clone(),
            }
        } else {
            TurnAwaited::Nothing
        }),
        State::Ended => Some(TurnAwaited::Nothing),
        State::Held { .. } => Some(TurnAwaited::Held),
        State::Waiting { .. } => None,
    }
}

/// Turns a stop still waiting for its send into a held one when its wait
/// ends, however it ends.
struct HoldOnLeave {
    key: String,
    user_id: String,
}

impl Drop for HoldOnLeave {
    fn drop(&mut self) {
        if let Ok(mut turns) = TURNS.lock()
            && let Some(e) = turns.get_mut(&self.key)
            && e.user_id == self.user_id
            && let State::Waiting { conversation_id } = &e.state
        {
            e.state = State::Held {
                conversation_id: conversation_id.clone(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn soon() -> Instant {
        Instant::now() + Duration::from_secs(5)
    }

    /// A fresh uuid-shaped name per call: the tests share one table.
    fn name() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        format!("7e57a11e-0000-4000-8000-{n:012x}")
    }

    const ME: &str = "a1b2c3d4-0000-4000-8000-000000000001";
    const SOMEONE: &str = "a1b2c3d4-0000-4000-8000-000000000002";
    const THREAD: &str = "a1b2c3d4-0000-4000-8000-0000000000aa";
    const OTHER_THREAD: &str = "a1b2c3d4-0000-4000-8000-0000000000bb";

    /// A stop that comes first waits, and goes on to the thread the send
    /// names; whichever spelling of the name either carried.
    #[tokio::test]
    async fn a_stop_that_beats_its_send_waits_for_it() {
        let turn = name();
        let waiting = {
            let turn = turn.to_ascii_uppercase();
            tokio::spawn(async move { await_sent_turn(&turn, ME, Some(THREAD), soon()).await })
        };
        tokio::task::yield_now().await;
        let sent = turn_sent(&turn, ME, &THREAD.to_ascii_uppercase());
        assert!(
            !sent.stop_held(),
            "the waiting stop is the route's, not held"
        );
        assert_eq!(
            waiting.await.unwrap(),
            TurnAwaited::Sent {
                conversation_id: THREAD.into()
            }
        );
        // A new thread's stop names no thread and takes the send's.
        let turn = name();
        let waiting = {
            let turn = turn.clone();
            tokio::spawn(async move { await_sent_turn(&turn, ME, None, soon()).await })
        };
        tokio::task::yield_now().await;
        let _sent = turn_sent(&turn, ME, OTHER_THREAD);
        assert_eq!(
            waiting.await.unwrap(),
            TurnAwaited::Sent {
                conversation_id: OTHER_THREAD.into()
            }
        );
        drop(sent);
    }

    /// Past its wait the stop is held, and the send that comes later is told
    /// to hand it on, once.
    #[tokio::test]
    async fn a_stop_outliving_its_wait_is_held_for_the_send() {
        let turn = name();
        let deadline = Instant::now() + Duration::from_millis(100);
        assert_eq!(
            await_sent_turn(&turn, ME, Some(THREAD), deadline).await,
            TurnAwaited::Held
        );
        // A second stop for the same turn finds it held.
        assert_eq!(
            await_sent_turn(&turn, ME, Some(THREAD), soon()).await,
            TurnAwaited::Held
        );
        let sent = turn_sent(&turn, ME, THREAD);
        assert!(
            sent.stop_held(),
            "the send hands the held stop to its opening"
        );
        // Now the send is here: a later stop goes straight on to it.
        assert_eq!(
            await_sent_turn(&turn, ME, Some(THREAD), soon()).await,
            TurnAwaited::Sent {
                conversation_id: THREAD.into()
            }
        );
        drop(sent);
        // And once it ends, a stop for it has nothing to stop, at once.
        let started = Instant::now();
        assert_eq!(
            await_sent_turn(&turn, ME, Some(THREAD), soon()).await,
            TurnAwaited::Nothing
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// Never another person's turn: a stop under a name someone else sent
    /// is nothing, and a stop someone else holds under a name is not handed
    /// to this person's send. Nor is a stop for another thread.
    #[tokio::test]
    async fn a_stop_never_reaches_another_persons_turn() {
        let turn = name();
        let _sent = turn_sent(&turn, ME, THREAD);
        assert_eq!(
            await_sent_turn(&turn, SOMEONE, None, soon()).await,
            TurnAwaited::Nothing
        );
        assert_eq!(
            await_sent_turn(&turn, SOMEONE, Some(THREAD), soon()).await,
            TurnAwaited::Nothing
        );

        let turn = name();
        let deadline = Instant::now() + Duration::from_millis(50);
        assert_eq!(
            await_sent_turn(&turn, SOMEONE, None, deadline).await,
            TurnAwaited::Held
        );
        let sent = turn_sent(&turn, ME, THREAD);
        assert!(!sent.stop_held(), "someone else's held stop is not mine");
        drop(sent);

        let turn = name();
        let deadline = Instant::now() + Duration::from_millis(50);
        assert_eq!(
            await_sent_turn(&turn, ME, Some(OTHER_THREAD), deadline).await,
            TurnAwaited::Held
        );
        let sent = turn_sent(&turn, ME, THREAD);
        assert!(
            !sent.stop_held(),
            "a stop for another thread is not this send's"
        );
        drop(sent);
    }

    /// A send that queued behind another turn lets go of its opening at
    /// once, and with it its name: a stop for it has nothing to stop.
    #[tokio::test]
    async fn a_queued_send_has_nothing_to_stop() {
        let turn = name();
        drop(turn_sent(&turn, ME, THREAD));
        assert_eq!(
            await_sent_turn(&turn, ME, Some(THREAD), soon()).await,
            TurnAwaited::Nothing
        );
    }

    /// A stop whose own request goes away before its send comes is held all
    /// the same.
    #[tokio::test]
    async fn a_stop_whose_request_leaves_is_held() {
        let turn = name();
        let waiting = {
            let turn = turn.clone();
            tokio::spawn(async move { await_sent_turn(&turn, ME, None, soon()).await })
        };
        tokio::task::yield_now().await;
        waiting.abort();
        let _ = waiting.await;
        let sent = turn_sent(&turn, ME, THREAD);
        assert!(sent.stop_held());
    }

    /// One person's held stops are bounded: past the bound the oldest goes.
    #[tokio::test]
    async fn held_stops_are_bounded_per_person() {
        let me = "a1b2c3d4-0000-4000-8000-0000000000cc";
        let first = name();
        let now = || Instant::now() + Duration::from_millis(1);
        assert_eq!(
            await_sent_turn(&first, me, None, now()).await,
            TurnAwaited::Held
        );
        for _ in 0..HELD_PER_USER {
            assert_eq!(
                await_sent_turn(&name(), me, None, now()).await,
                TurnAwaited::Held
            );
        }
        let sent = turn_sent(&first, me, THREAD);
        assert!(!sent.stop_held(), "the oldest held stop was let go");
        drop(sent);
    }
}
