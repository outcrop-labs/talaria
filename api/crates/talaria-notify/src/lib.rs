// Notifications: the user's inbox (list, unread count, mark-read), the
// per-user routing table in users.notify_prefs, the instance email master
// switch in app_settings, the single writer (`add_notification`) every filing
// of a row goes through, and the mail plane — the one door out, the outbox,
// the breaker, and the drain the scheduler ticks. The transport itself
// (SMTP/Resend) and the shared mail chrome live in email.rs; the digest
// builds on the same door.
//
// The class vocabulary — classes, fallbacks, the digest's reserved key — is
// declared here once; resolve/digest DERIVE the effective answer from the
// stored jsonb, so a hand-edited row degrades to the same defaults.
//
// The crate root holds only the module list, the re-exports (every caller
// names `talaria_notify::<item>` and compiles untouched), and `NotifyDeps`.
// Each module has one owner; see docs/notifications/contract.md.

use sqlx::PgPool;
use talaria_realtime::RealtimeDeps;

pub mod brief;
pub mod classes;
pub mod deliver;
pub mod href;
pub mod inbox;
pub mod mail;
pub mod prefs;
pub mod threads;
pub mod write;

pub use brief::*;
pub use classes::*;
pub use href::*;
pub use inbox::*;
pub use mail::*;
pub use prefs::*;
pub use threads::*;
pub use write::*;

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
