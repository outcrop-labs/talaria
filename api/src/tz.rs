// The one true `localMoment` — port of ui/src/server/tz.ts, kept as its own
// leaf for the same reason the TS file is: the brief's config and (next slice)
// the digest both read zones through it, and sharing one copy must not drag
// either module's graph into the other.

use chrono::TimeZone;

/// Local hour and calendar date in a zone.
///
/// Read through the zone itself rather than by offset arithmetic: stepping a
/// wall clock by UTC math is wrong across a DST boundary — the day a zone
/// springs forward has no 02:00, the day it falls back has two — and asking
/// the zone what its wall clock says is the version that survives both.
/// `next_brief_at` walks an hour at a time through this function for exactly
/// that reason.
///
/// A bad zone falls back to UTC instead of throwing: a typo in one settings
/// row must not take down a scheduled job for the whole workspace — the digest
/// and the brief both read org- and per-person zones that nobody validates at
/// rest, and "one bad row silences everyone's brief" is the failure mode both
/// files exist to prevent. It says so and carries on.
pub fn local_moment(time_zone: &str, at_ms: i64) -> LocalMoment {
    match time_zone.parse::<chrono_tz::Tz>() {
        Ok(zone) => read(zone, at_ms),
        Err(_) => {
            tracing::warn!("[tz] unknown time zone \"{time_zone}\" — falling back to UTC");
            read(chrono_tz::UTC, at_ms)
        }
    }
}

/// The answer, in the shape every caller asks for. `hour` is 0-23 local.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMoment {
    pub hour: u32,
    /// Minutes past the hour — the brief's schedule slot renders it
    /// ("7:30 PM"), and nothing else on this plane reads at minute grain.
    pub minute: u32,
    pub date: String,
    pub zone: String,
}

fn read(zone: chrono_tz::Tz, at_ms: i64) -> LocalMoment {
    // `timestamp_millis_opt` is the infallible-spelling version of the chrono
    // constructor; an out-of-range instant cannot arrive from a millisecond
    // count this side of the heat death of the universe, but the Opt still
    // gets an answer rather than a panic. `DateTime<Utc>` is the one DateTime
    // with a Default — the epoch — so that is the floor.
    let local = zone
        .timestamp_millis_opt(at_ms)
        .single()
        .unwrap_or_else(|| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at_ms)
                .unwrap_or_default()
                .with_timezone(&zone)
        });
    LocalMoment {
        hour: local.format("%H").to_string().parse().unwrap_or(0),
        minute: local.format("%M").to_string().parse().unwrap_or(0),
        date: local.format("%Y-%m-%d").to_string(),
        zone: zone.name().to_string(),
    }
}

/// `Date.parse` for the calendar's clock shapes: RFC3339 with a `Z` or
/// `±HH:MM` offset, and a bare `YYYY-MM-DD` (which `new Date` reads as UTC
/// midnight — the all-day-event spelling). Anything else is None; callers
/// treat unparseable the way TS treats NaN.
pub fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() == 10 {
        // Date-only: UTC midnight, exactly as `new Date('YYYY-MM-DD')`.
        return crate::agent_auth::iso_to_epoch_ms(&format!("{s}T00:00:00.000Z"));
    }
    // OFFSET FORMS FIRST: `iso_to_epoch_ms` accepts a `±HH:MM` tail but reads
    // the clock as UTC whatever it says, so a passthrough here would silently
    // drop the offset — four hours off a New York afternoon, the exact class
    // of wrong this function exists to prevent. Only a literal `Z` goes
    // straight through.
    if b.len() >= 25
        && let Some(&sign_byte) = b.get(b.len() - 6)
        && (sign_byte == b'+' || sign_byte == b'-')
    {
        let sign = if sign_byte == b'+' { 1i64 } else { -1i64 };
        let digits = |from: usize, to: usize| -> Option<i64> {
            let mut v: i64 = 0;
            for &c in b.get(from..to)? {
                if !c.is_ascii_digit() {
                    return None;
                }
                v = v * 10 + (c - b'0') as i64;
            }
            Some(v)
        };
        let oh = digits(b.len() - 5, b.len() - 3)?;
        let om = digits(b.len() - 2, b.len())?;
        let mut naive = s[..b.len() - 6].to_string();
        naive.push('Z');
        return crate::agent_auth::iso_to_epoch_ms(&naive)
            .map(|ms| ms - sign * (oh * 3_600_000 + om * 60_000));
    }
    crate::agent_auth::iso_to_epoch_ms(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-03-08 07:30 UTC is 02:30 in New York — the hour that DOES NOT
    // EXIST that day (the zone sprang forward at 02:00). The zone itself
    // answers 03:30; offset arithmetic would have said 02:30 and been wrong.
    #[test]
    fn reads_through_the_zone_across_dst_spring_forward() {
        let m = local_moment("America/New_York", 1_772_955_000_000);
        assert_eq!((m.hour, m.date.as_str()), (3, "2026-03-08"));
    }

    // The fall-back day has two 01:00s; the zone disambiguates by offset.
    // 2026-11-01 05:30 UTC = 01:30 EDT (the second 01:00's approach).
    #[test]
    fn reads_through_the_zone_across_dst_fall_back() {
        let m = local_moment("America/New_York", 1_793_511_000_000);
        assert_eq!((m.hour, m.date.as_str()), (1, "2026-11-01"));
    }

    #[test]
    fn an_unknown_zone_falls_back_to_utc_and_says_so() {
        let m = local_moment("Not/AZone", 0);
        assert_eq!(
            (m.hour, m.date.as_str(), m.zone.as_str()),
            (0, "1970-01-01", "UTC")
        );
    }

    #[test]
    fn utc_reads_are_exact() {
        // 2026-08-29 23:17 UTC.
        let m = local_moment("UTC", 1_788_045_420_000);
        assert_eq!((m.hour, m.minute, m.date.as_str()), (23, 17, "2026-08-29"));
    }

    #[test]
    fn parses_the_calendars_clock_shapes() {
        // Z-suffixed, offset-suffixed, with and without millis, and the
        // date-only all-day spelling (UTC midnight, as `new Date` reads it).
        assert_eq!(
            parse_rfc3339_ms("2026-08-29T23:17:00Z"),
            Some(1_788_045_420_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-08-29T23:17:00.000Z"),
            Some(1_788_045_420_000)
        );
        // 23:17Z written with the New York offset — every spelling of this
        // list is the SAME instant, so a spelling that lands elsewhere is a
        // parse bug, not a rounding disagreement.
        assert_eq!(
            parse_rfc3339_ms("2026-08-29T19:17:00-04:00"),
            Some(1_788_045_420_000)
        );
        assert_eq!(
            parse_rfc3339_ms("2026-08-30T02:17:00+03:00"),
            Some(1_788_045_420_000)
        );
        assert_eq!(parse_rfc3339_ms("2026-08-29"), Some(1_787_961_600_000)); // UTC midnight
        assert_eq!(parse_rfc3339_ms("2026-08-29T14:17"), None);
        assert_eq!(parse_rfc3339_ms("not a date"), None);
    }
}
