// Notifications: the person's inbox, the single writer every filing of a row
// goes through, the class registry and per-user settings, presence, the ack,
// and the mail plane. The contract the wave-2 streams build on is
// docs/notifications/contract.md (I-10 … I-24).
//
// The crate root holds only the module list, the re-exports (every caller
// names `talaria_notify::<item>` and compiles untouched), `NotifyDeps` and
// `features()`. Each module has one owner in wave 2:
//
//   lib, meta, href, brief          frozen (the contract)
//   classes, prefs, write, deliver,
//   presence, defer, mail, retention  W2-E (engine)
//   inbox                            W2-H (hub and inbox API)
//   threads                          W2-P (producers)

use sqlx::PgPool;
use talaria_realtime::RealtimeDeps;

pub mod agent_mutes;
pub mod brief;
pub mod classes;
pub mod defer;
pub mod deliver;
pub mod href;
pub mod inbox;
pub mod mail;
pub mod meta;
pub mod prefs;
pub mod presence;
pub mod retention;
pub mod threads;
pub mod write;

pub use agent_mutes::*;
pub use brief::*;
pub use classes::*;
pub use href::*;
pub use inbox::*;
pub use mail::*;
pub use meta::*;
pub use prefs::*;
pub use threads::*;
pub use write::*;

/// The contract's own feature flags (I-23), what phase 0 serves: the seq
/// cursor, the class registry, the channel matrix, the stored DND and
/// presence, the ack, the devices list, read ids on the read-sync event, and
/// the stored channel levels. Clients enable UI on a feature, never on a
/// version number.
pub const FEATURES: &[&str] = &[
    "seq",
    "classes",
    "channels",
    "dnd-store",
    "presence-store",
    "ack",
    "devices",
    "read-ids",
    "channel-levels-store",
];

/// Every feature this api serves (`features` in `GET /api/notifications`):
/// the contract's, then each module's own list — the delivery engine, the
/// presence plane, the inbox, the thread producers. Each stream turns its
/// features on in its own file.
pub fn features() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = FEATURES.to_vec();
    for f in deliver::FEATURES
        .iter()
        .chain(presence::FEATURES)
        .chain(inbox::FEATURES)
        .chain(threads::FEATURES)
    {
        if !out.contains(f) {
            out.push(f);
        }
    }
    out
}

/// The edges a notification write touches beyond its own row: the pool the row
/// lands in and the realtime fan-out that tells the person's open pages.
#[derive(Clone)]
pub struct NotifyDeps {
    pub pg: PgPool,
    pub realtime: RealtimeDeps,
}

impl NotifyDeps {
    /// The publish half over the shared manager, from the pieces every route
    /// already holds. Redis unreachable at construction is logged inside
    /// `publish_only` and publishes nothing — the row is still the record.
    pub fn publishing(pg: PgPool, conn: Option<redis::aio::ConnectionManager>) -> Self {
        Self {
            pg,
            realtime: RealtimeDeps::publish_only(conn),
        }
    }
}
