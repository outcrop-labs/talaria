// When the daily brief opens, and where that answer comes from — port of
// ui/src/server/daily-brief-config.ts.
//
// THE RULE IS "TWO HOURS BEFORE NORMAL WORKING HOURS", which is a product
// decision with a clock attached, so it is stated as a workday start and a lead
// rather than as a fire hour. Those are two different facts: an org that moves
// to an 08:00 start wants its brief at 06:00 without anyone recomputing, and an
// org that wants more (or less) reading time before the day starts is changing
// the LEAD, not the start. Storing only the answer would have made both of
// those edits look identical, and the second one silently wrong.
//
// THE ZONE IS PER-PERSON: `users.timezone` when the account set one, this
// config's zone (TZ env → UTC) otherwise.

use crate::gateway::settings::get_setting;
use crate::tz::local_moment;
use sqlx::PgPool;

pub const BRIEF_CONFIG_KEY: &str = "brief_config";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefConfig {
    /// Local hour (0-23) the workday is considered to start.
    pub workday_start_hour: i64,
    /// How many hours BEFORE that the brief opens. The product rule is 2.
    pub lead_hours: i64,
    /// IANA zone the hours are read in.
    pub time_zone: String,
    /// Minutes between sweeps of an open brief. The floor on "how stale can the
    /// brief be", and the ceiling on how much work following the day costs — a
    /// sweep is four scoped queries per person with an open brief.
    pub sweep_minutes: i64,
}

fn default_zone() -> String {
    // `process.env.TZ || 'UTC'` — an empty TZ env is unset for these purposes.
    std::env::var("TZ")
        .ok()
        .filter(|z| !z.is_empty())
        .unwrap_or_else(|| "UTC".to_string())
}

fn defaults() -> BriefConfig {
    BriefConfig {
        workday_start_hour: 9,
        lead_hours: 2,
        time_zone: default_zone(),
        // Five minutes. The brief is the surface a person leaves open all day,
        // and a slower sweep is directly visible as "it did not know yet".
        // Realtime nudges shorten the observed latency further for the events
        // that publish; this is the floor for everything that does not.
        sweep_minutes: 5,
    }
}

/// The hour a config value must be to count: an integer 0-23.
fn hour_in(value: Option<&serde_json::Value>, fallback: i64) -> i64 {
    match value.and_then(|v| v.as_i64()) {
        Some(h) if (0..=23).contains(&h) => h,
        _ => fallback,
    }
}

fn positive(value: Option<&serde_json::Value>, fallback: i64) -> i64 {
    match value.and_then(|v| v.as_i64()) {
        Some(v) if v > 0 => v,
        _ => fallback,
    }
}

/// The stored config merged over the defaults, every field defended — the
/// settings row is admin-editable JSON and no field of it is trusted raw.
pub async fn brief_config(pg: &PgPool) -> BriefConfig {
    let stored = get_setting(
        pg,
        BRIEF_CONFIG_KEY,
        serde_json::Value::Object(Default::default()),
    )
    .await;
    BriefConfig {
        workday_start_hour: hour_in(
            stored.get("workdayStartHour"),
            defaults().workday_start_hour,
        ),
        // Clamped to the day it is subtracted from. A lead of 30 would
        // otherwise wrap `fire_hour` into a negative and open every brief at a
        // nonsense hour; the config surface is admin-editable and this is the
        // value most likely to be typed wrong.
        lead_hours: positive(stored.get("leadHours"), defaults().lead_hours).min(23),
        time_zone: stored
            .get("timeZone")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|z| !z.is_empty())
            .map(str::to_string)
            .unwrap_or_else(default_zone),
        sweep_minutes: positive(stored.get("sweepMinutes"), defaults().sweep_minutes).max(1),
    }
}

/// The local hour a brief opens. Wraps rather than clamps: a 01:00 start with a
/// 2h lead means the brief opens at 23:00 the PREVIOUS evening, which is the
/// literally correct reading of "two hours before you start" for a night shift
/// and the only one that does not silently move someone's brief to midnight.
pub fn fire_hour(config: &BriefConfig) -> i64 {
    (config.workday_start_hour - config.lead_hours + 24) % 24
}

/// The person's zone: their stored preference when they set one, the
/// workspace's otherwise. Pure over the VALUE (not the user id) on purpose —
/// callers that hold only an id fetch it first, and the scheduled pass carries
/// it in the same query as the user row. A blank stored value counts as unset,
/// so a hand-emptied row follows the workspace rather than throwing at every
/// format call.
pub fn zone_for(stored: Option<&str>, workspace_zone: &str) -> String {
    match stored.map(str::trim).filter(|s| !s.is_empty()) {
        Some(zone) => zone.to_string(),
        None => workspace_zone.to_string(),
    }
}

/// Local hour in a zone. An unknown zone falls back to UTC rather than throwing
/// (see `local_moment`).
pub fn local_hour(zone: &str, at_ms: i64) -> i64 {
    local_moment(zone, at_ms).hour as i64
}

/// Next moment a brief opens in `zone`, from `at_ms` (epoch ms). Rendered by
/// the surface while it is waiting, so "no brief yet" can say WHEN instead of
/// just NO.
pub fn next_brief_at(config: &BriefConfig, zone: &str, at_ms: i64) -> i64 {
    let hour = fire_hour(config);
    // Walked forward an hour at a time rather than computed, because arithmetic
    // on a wall clock is wrong across a DST boundary — the day a zone springs
    // forward has no 02:00, and the day it falls back has two. Stepping and
    // re-reading the local hour is the version that survives both.
    let mut step = at_ms - at_ms.rem_euclid(3_600_000);
    for _ in 0..=48 {
        step += 3_600_000;
        if local_hour(zone, step) == hour {
            return step;
        }
    }
    step
}

/// Which workday a brief opened at `at_ms` is FOR, and whether it is due yet.
///
/// ONE FUNCTION FOR BOTH ANSWERS, because they are the same question asked
/// twice and the two ways of getting them wrong compound. `due` is a WINDOW,
/// not an equality: the scheduler ticks every few minutes and can miss one — a
/// deploy, a lease held by an instance that died, a slow pass — and an
/// `hour == fire_hour` test would mean one missed tick costs somebody their
/// entire brief for the day. This says "the hour has come and the workday has
/// not turned over", so the next tick after any gap still opens it.
///
/// `date` is the workday the brief COVERS, which is not always the local date
/// it opens on. With an early enough start the lead wraps the fire hour into
/// the previous evening (a 01:00 start, 2h lead → the brief opens at 23:00),
/// and a brief written on Monday evening for Tuesday's work belongs to Tuesday.
/// Getting this wrong does not merely mislabel the document — `brief_date` is
/// half the unique key, so an evening brief filed under the wrong day would be
/// re-opened, as a second brief, when the day it was written for arrived.
pub fn brief_window(config: &BriefConfig, zone: &str, at_ms: i64) -> BriefWindow {
    let moment = local_moment(zone, at_ms);
    let fire = fire_hour(config);
    let wrapped = fire > config.workday_start_hour;
    if !wrapped {
        return BriefWindow {
            due: moment.hour as i64 >= fire,
            date: moment.date,
        };
    }
    // Wrapped: the window is [fire, midnight) ∪ [midnight, workdayStart), and
    // the first half is the evening BEFORE the day it is for.
    if moment.hour as i64 >= fire {
        return BriefWindow {
            due: true,
            date: shift_date(&moment.date, 1),
        };
    }
    BriefWindow {
        due: (moment.hour as i64) < config.workday_start_hour,
        date: moment.date,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefWindow {
    pub due: bool,
    pub date: String,
}

/// Calendar-date arithmetic on a `YYYY-MM-DD` string. Done on a parsed date
/// rather than on the zone, because the input is already a LOCAL date and "the
/// day after Monday" is a calendar fact with no clock in it — running it
/// through a zone again is what introduces a DST bug rather than avoiding one.
fn shift_date(date: &str, days: i64) -> String {
    use chrono::Datelike;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| d + chrono::Duration::days(days))
        .map(|d| format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day()))
        // An unparseable date is a bug upstream of this function; the identity
        // keeps it visible as a wrong label rather than taking the scheduler
        // down mid-pass.
        .unwrap_or_else(|_| date.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every assertion here is a way somebody loses a brief for a day: a fire
    // hour computed wrong, a window that only opens on the exact minute the
    // scheduler happens to tick, or an evening brief filed under the wrong date
    // and therefore re-opened as a second one the next morning.
    fn base() -> BriefConfig {
        BriefConfig {
            workday_start_hour: 9,
            lead_hours: 2,
            time_zone: "UTC".into(),
            sweep_minutes: 5,
        }
    }
    fn defaults_workday(v: i64) -> BriefConfig {
        BriefConfig {
            workday_start_hour: v,
            ..base()
        }
    }

    fn ms(iso: &str) -> i64 {
        crate::agent_auth::iso_to_epoch_ms(iso).expect("test iso parses")
    }

    #[test]
    fn fire_hour_is_the_workday_start_less_the_lead() {
        assert_eq!(fire_hour(&base()), 7);
    }

    #[test]
    fn fire_hour_wraps_into_the_previous_evening_rather_than_clamping() {
        // A 01:00 start with a 2h lead means 23:00 the night before. Clamping
        // to 0 would silently move a night-shift brief to midnight, which is
        // neither what was configured nor two hours before anything.
        assert_eq!(fire_hour(&defaults_workday(1)), 23);
    }

    #[test]
    fn window_is_not_due_before_the_fire_hour() {
        assert!(!brief_window(&base(), "UTC", ms("2026-08-17T06:59:00Z")).due);
    }

    #[test]
    fn window_stays_due_for_the_rest_of_the_day() {
        // THE POINT OF A WINDOW RATHER THAN AN EQUALITY. The scheduler can miss
        // a tick — a deploy, a dead lease — and an `hour == fire_hour` test
        // would cost that person their whole brief.
        for at in [
            "2026-08-17T07:00:00Z",
            "2026-08-17T15:30:00Z",
            "2026-08-17T23:59:00Z",
        ] {
            assert!(brief_window(&base(), "UTC", ms(at)).due, "{at}");
        }
    }

    #[test]
    fn dates_an_ordinary_brief_with_the_local_day_it_opens_on() {
        assert_eq!(
            brief_window(&base(), "UTC", ms("2026-08-17T07:30:00Z")).date,
            "2026-08-17"
        );
    }

    #[test]
    fn dates_a_wrapped_evening_brief_with_the_workday_it_is_for() {
        let wrapped = defaults_workday(1);
        // Written Monday 23:00, for Tuesday. `brief_date` is half the unique
        // key, so filing this under Monday would have it re-opened as a SECOND
        // brief when Tuesday actually arrived.
        assert_eq!(
            brief_window(&wrapped, "UTC", ms("2026-08-17T23:10:00Z")).date,
            "2026-08-18"
        );
        assert_eq!(
            brief_window(&wrapped, "UTC", ms("2026-08-18T00:30:00Z")),
            BriefWindow {
                due: true,
                date: "2026-08-18".into()
            }
        );
    }

    #[test]
    fn closes_the_wrapped_window_once_the_workday_starts() {
        assert!(!brief_window(&defaults_workday(1), "UTC", ms("2026-08-18T02:00:00Z")).due);
    }

    #[test]
    fn reads_the_hour_in_the_persons_zone_not_the_servers() {
        // 07:00 in New York is 11:00 UTC. A window computed on the server clock
        // would open this brief four hours early and title it correctly, which
        // is the failure that looks like it works.
        assert!(!brief_window(&base(), "America/New_York", ms("2026-08-17T10:59:00Z")).due);
        assert!(brief_window(&base(), "America/New_York", ms("2026-08-17T11:01:00Z")).due);
    }

    #[test]
    fn falls_back_to_utc_on_an_unreadable_zone_instead_of_throwing() {
        // A typo in one org-wide settings row must not stop every brief in the
        // workspace — that is the exact failure the scheduler exists to prevent.
        assert_eq!(
            brief_window(&base(), "Not/AZone", ms("2026-08-17T07:30:00Z")),
            BriefWindow {
                due: true,
                date: "2026-08-17".into()
            }
        );
    }

    #[test]
    fn next_is_the_next_occurrence_of_the_fire_hour() {
        assert_eq!(
            next_brief_at(&base(), "UTC", ms("2026-08-17T06:10:00Z")),
            ms("2026-08-17T07:00:00Z")
        );
    }

    #[test]
    fn next_rolls_to_tomorrow_once_todays_has_passed() {
        assert_eq!(
            next_brief_at(&base(), "UTC", ms("2026-08-17T09:00:00Z")),
            ms("2026-08-18T07:00:00Z")
        );
    }

    #[test]
    fn next_lands_on_the_right_wall_clock_hour_across_dst() {
        // US DST ends 2026-11-01. Walking the clock an hour at a time and
        // re-reading the LOCAL hour is what makes this land on 07:00 local
        // either side of it; adding 24h of milliseconds would drift by the hour
        // the zone gave back.
        let next = next_brief_at(&base(), "America/New_York", ms("2026-10-31T20:00:00Z"));
        assert_eq!(local_moment("America/New_York", next).hour, 7);
    }

    #[test]
    fn zone_for_lets_the_stored_zone_win() {
        assert_eq!(
            zone_for(Some("Asia/Tokyo"), &base().time_zone),
            "Asia/Tokyo"
        );
    }

    #[test]
    fn zone_for_follows_the_workspace_when_nothing_real_is_stored() {
        let denver = BriefConfig {
            time_zone: "America/Denver".into(),
            ..base()
        };
        for unset in [None, Some(""), Some("   ")] {
            assert_eq!(zone_for(unset, &denver.time_zone), "America/Denver");
        }
    }
}
