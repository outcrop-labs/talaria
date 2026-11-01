// Cross-device presence (I-17): which of a person's clients is the one they
// are on. Each client heartbeats `PUT /api/me/presence` with its own id, kind
// and state; the store keeps one Redis hash per person, the winner rule picks
// the client to alert on, and a `presence` event goes out on `user:<id>` only
// when that answer changes.
//
// Phase 0 stores and announces; nothing gates on it yet. W2-E gates the push
// on it (docs/notifications/delivery.md); the thresholds are consts it may
// tune.

use redis::AsyncCommands;
use serde_json::{Value, json};
use talaria_realtime::{UserEvent, publish_user};

use crate::NotifyDeps;

/// The presence plane's own feature flags (I-23). W2-E adds its own here.
pub const FEATURES: &[&str] = &[];

/// The hash's TTL: a person whose every client went silent has no presence.
pub const PRESENCE_TTL_SECS: i64 = 120;
/// An entry older than this is pruned on read (a client heartbeats every
/// 30 s, so 75 s is two missed beats and a margin).
pub const ENTRY_STALE_MS: i64 = 75_000;

/// What kind of client reported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientKind {
    Web,
    Desktop,
}

/// The state a client computes for itself. `looking`: visible, focused and
/// touched in the last 60 s. `active`: (web) visible and touched in the last
/// 120 s, (desktop) OS input in the last 120 s. Otherwise `idle` or `hidden`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientState {
    Looking,
    Active,
    Idle,
    Hidden,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientKind::Web => "web",
            ClientKind::Desktop => "desktop",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "web" => Some(ClientKind::Web),
            "desktop" => Some(ClientKind::Desktop),
            _ => None,
        }
    }
}

impl ClientState {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientState::Looking => "looking",
            ClientState::Active => "active",
            ClientState::Idle => "idle",
            ClientState::Hidden => "hidden",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "looking" => Some(ClientState::Looking),
            "active" => Some(ClientState::Active),
            "idle" => Some(ClientState::Idle),
            "hidden" => Some(ClientState::Hidden),
            _ => None,
        }
    }
}

/// One heartbeat, as the route validated it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heartbeat {
    /// `[A-Za-z0-9_-]{8,64}`, chosen by the client and stable for its life.
    pub client_id: String,
    pub kind: ClientKind,
    pub state: ClientState,
    pub os_idle_ms: Option<u64>,
}

/// A client id the store accepts.
pub fn valid_client_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One stored client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub client_id: String,
    pub kind: ClientKind,
    pub state: ClientState,
    pub at_ms: i64,
    pub os_idle_ms: Option<u64>,
}

/// The winner pair: the client the person is looking at, else the one they
/// are active on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Presence {
    pub looking: Option<String>,
    pub active: Option<String>,
}

/// The winner rule. Pure: the most recent `looking` client wins `looking`;
/// with none, the most recent `active` one wins `active`. Stale entries do not
/// count.
pub fn winner(entries: &[Entry], now_ms: i64) -> Presence {
    let fresh = |e: &&Entry| now_ms - e.at_ms <= ENTRY_STALE_MS;
    let newest = |state: ClientState| {
        entries
            .iter()
            .filter(fresh)
            .filter(|e| e.state == state)
            .max_by(|a, b| a.at_ms.cmp(&b.at_ms).then(a.client_id.cmp(&b.client_id)))
            .map(|e| e.client_id.clone())
    };
    let looking = newest(ClientState::Looking);
    let active = if looking.is_some() {
        None
    } else {
        newest(ClientState::Active)
    };
    Presence { looking, active }
}

fn dev_key(user_id: &str) -> String {
    format!("presence:dev:{user_id}")
}

fn win_key(user_id: &str) -> String {
    format!("presence:win:{user_id}")
}

fn entry_json(h: &Heartbeat, at_ms: i64) -> String {
    json!({ "k": h.kind.as_str(), "s": h.state.as_str(), "at": at_ms, "i": h.os_idle_ms })
        .to_string()
}

fn parse_entry(client_id: &str, raw: &str) -> Option<Entry> {
    let v: Value = serde_json::from_str(raw).ok()?;
    Some(Entry {
        client_id: client_id.to_string(),
        kind: ClientKind::parse(v.get("k")?.as_str()?)?,
        state: ClientState::parse(v.get("s")?.as_str()?)?,
        at_ms: v.get("at")?.as_i64()?,
        os_idle_ms: v.get("i").and_then(Value::as_u64),
    })
}

/// The person's live entries; stale and unreadable ones are deleted on the way.
async fn read_entries(
    conn: &mut redis::aio::ConnectionManager,
    user_id: &str,
    now_ms: i64,
) -> redis::RedisResult<Vec<Entry>> {
    let raw: Vec<(String, String)> = conn.hgetall(dev_key(user_id)).await?;
    let mut live = Vec::new();
    let mut dead = Vec::new();
    for (id, v) in raw {
        match parse_entry(&id, &v) {
            Some(e) if now_ms - e.at_ms <= ENTRY_STALE_MS => live.push(e),
            _ => dead.push(id),
        }
    }
    if !dead.is_empty() {
        let _: i64 = conn.hdel(dev_key(user_id), dead).await?;
    }
    live.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then(a.client_id.cmp(&b.client_id)));
    Ok(live)
}

/// Record one heartbeat and announce the winner when it changed. Answers the
/// winner after it.
pub async fn record(
    deps: &NotifyDeps,
    conn: &mut redis::aio::ConnectionManager,
    user_id: &str,
    h: &Heartbeat,
    now_ms: i64,
) -> redis::RedisResult<Presence> {
    let key = dev_key(user_id);
    let _: i64 = conn.hset(&key, &h.client_id, entry_json(h, now_ms)).await?;
    let _: bool = conn.expire(&key, PRESENCE_TTL_SECS).await?;
    let entries = read_entries(conn, user_id, now_ms).await?;
    let win = winner(&entries, now_ms);
    let pair = json!([win.looking, win.active]).to_string();
    let prev: Option<String> = conn.get(win_key(user_id)).await?;
    // The cache's own TTL is refreshed every beat, so a winner that holds for
    // an hour is announced once, not every two minutes.
    let _: () = conn
        .set_ex(win_key(user_id), &pair, PRESENCE_TTL_SECS as u64)
        .await?;
    if prev.as_deref() != Some(pair.as_str()) {
        publish_user(
            &deps.realtime,
            user_id,
            &UserEvent::Presence {
                looking: win.looking.clone(),
                active: win.active.clone(),
                clients: entries.len() as u32,
            },
        );
    }
    Ok(win)
}

/// `GET /api/me/presence`: the winner pair and every live client, newest
/// first.
pub async fn snapshot(
    conn: &mut redis::aio::ConnectionManager,
    user_id: &str,
    now_ms: i64,
) -> redis::RedisResult<Value> {
    let entries = read_entries(conn, user_id, now_ms).await?;
    let win = winner(&entries, now_ms);
    Ok(json!({
        "looking": win.looking,
        "active": win.active,
        "clients": entries.iter().map(|e| json!({
            "clientId": e.client_id,
            "kind": e.kind.as_str(),
            "state": e.state.as_str(),
            "at": talaria_agent_auth::epoch_ms_to_iso(e.at_ms),
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, state: ClientState, at: i64) -> Entry {
        Entry {
            client_id: id.into(),
            kind: ClientKind::Web,
            state,
            at_ms: at,
            os_idle_ms: None,
        }
    }

    #[test]
    fn the_winner_is_the_newest_looking_else_the_newest_active() {
        let now = 1_000_000;
        let all = [
            e("tab-aaaaaaaa", ClientState::Looking, now - 5_000),
            e("tab-bbbbbbbb", ClientState::Looking, now - 1_000),
            e("desk-cccccccc", ClientState::Active, now),
            e("tab-dddddddd", ClientState::Idle, now),
        ];
        assert_eq!(
            winner(&all, now),
            Presence {
                looking: Some("tab-bbbbbbbb".into()),
                active: None
            }
        );
        let none_looking = [
            e("desk-cccccccc", ClientState::Active, now - 2_000),
            e("tab-eeeeeeee", ClientState::Active, now - 1_000),
            e("tab-dddddddd", ClientState::Hidden, now),
        ];
        assert_eq!(
            winner(&none_looking, now),
            Presence {
                looking: None,
                active: Some("tab-eeeeeeee".into())
            }
        );
        // A stale looking client does not win.
        let stale = [e(
            "tab-aaaaaaaa",
            ClientState::Looking,
            now - ENTRY_STALE_MS - 1,
        )];
        assert_eq!(winner(&stale, now), Presence::default());
        assert_eq!(winner(&[], now), Presence::default());
    }

    #[test]
    fn client_ids_and_entries_are_checked() {
        assert!(valid_client_id("tab-A_b-12"));
        assert!(!valid_client_id("short"));
        assert!(!valid_client_id("has space in it"));
        assert!(!valid_client_id(&"x".repeat(65)));
        let h = Heartbeat {
            client_id: "tab-aaaaaaaa".into(),
            kind: ClientKind::Desktop,
            state: ClientState::Active,
            os_idle_ms: Some(1200),
        };
        let parsed = parse_entry("tab-aaaaaaaa", &entry_json(&h, 42)).unwrap();
        assert_eq!(parsed.kind, ClientKind::Desktop);
        assert_eq!(parsed.state, ClientState::Active);
        assert_eq!((parsed.at_ms, parsed.os_idle_ms), (42, Some(1200)));
        assert!(parse_entry("x", "not json").is_none());
    }
}
