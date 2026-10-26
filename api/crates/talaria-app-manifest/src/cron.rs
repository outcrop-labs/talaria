// A trigger's `cron`: the five-field schedule an app author already knows,
// read in UTC. The manifest validator refuses one that does not parse, with
// the sentence below; the trigger scheduler (talaria-app-triggers) asks a
// parsed one which minute was last due and which comes next.
//
// THE GRAMMAR, the classic crontab's without names:
//
//   minute hour day-of-month month day-of-week
//   0-59   0-23 1-31         1-12  0-7 (0 and 7 are Sunday)
//
// Each field is `*` or a comma list of `n`, `a-b`, `*/s`, `a-b/s` or `a/s`
// (from a to the field's end, every s). `@hourly`, `@daily` (`@midnight`),
// `@weekly`, `@monthly` and `@yearly` (`@annually`) stand for their usual
// expansions. Month and weekday NAMES are not read: a number cannot be
// misspelled into another day.
//
// THE DAY RULE IS VIXIE CRON'S: when both day fields are restricted (neither
// starts with `*`), a day matches if EITHER does — `0 9 1 * 1` is the first
// of the month and every Monday. Otherwise both must.
//
// The TS host's manifest twin (ui/src/server/app-build/manifest.ts
// `parseCron`) reads the same grammar and refuses with the same sentences,
// pinned by the shared fixture's cron cases.

const MINUTE_MS: i64 = 60_000;
const DAY_MINUTES: i64 = 1_440;

/// A parsed schedule. Each field is a bitset of the values it allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    /// The day-of-month field started with `*` — see the day rule above.
    days_star: bool,
    weekdays_star: bool,
}

struct Field {
    name: &'static str,
    min: u32,
    max: u32,
}

const FIELDS: [Field; 5] = [
    Field {
        name: "minute",
        min: 0,
        max: 59,
    },
    Field {
        name: "hour",
        min: 0,
        max: 23,
    },
    Field {
        name: "day of month",
        min: 1,
        max: 31,
    },
    Field {
        name: "month",
        min: 1,
        max: 12,
    },
    Field {
        name: "day of week",
        min: 0,
        max: 7,
    },
];

fn macro_of(s: &str) -> Option<&'static str> {
    Some(match s {
        "@yearly" | "@annually" => "0 0 1 1 *",
        "@monthly" => "0 0 1 * *",
        "@weekly" => "0 0 * * 0",
        "@daily" | "@midnight" => "0 0 * * *",
        "@hourly" => "0 * * * *",
        _ => return None,
    })
}

fn number(field: &Field, s: &str) -> Result<u32, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!(
            "the {} field has \"{s}\" where a number belongs",
            field.name
        ));
    }
    let n: u32 = s
        .parse()
        .map_err(|_| format!("the {} field's {s} is out of range", field.name))?;
    if n < field.min || n > field.max {
        return Err(format!(
            "the {} field's {n} is outside {}-{}",
            field.name, field.min, field.max
        ));
    }
    Ok(n)
}

/// One field's allowed values, as a bitset over 0..=63.
fn parse_field(field: &Field, text: &str) -> Result<u64, String> {
    let mut bits = 0u64;
    for item in text.split(',') {
        if item.is_empty() {
            return Err(format!("the {} field has an empty list item", field.name));
        }
        let (range, step) = match item.split_once('/') {
            Some((r, s)) => {
                let step = s.parse::<u32>().ok().filter(|n| *n >= 1).ok_or_else(|| {
                    format!(
                        "the {} field's step \"{s}\" is not a whole number of at least 1",
                        field.name
                    )
                })?;
                (r, Some(step))
            }
            None => (item, None),
        };
        let (lo, hi) = if range == "*" {
            (field.min, field.max)
        } else if let Some((a, b)) = range.split_once('-') {
            let (a, b) = (number(field, a)?, number(field, b)?);
            if a > b {
                return Err(format!(
                    "the {} field's range {a}-{b} runs backwards",
                    field.name
                ));
            }
            (a, b)
        } else {
            let a = number(field, range)?;
            // `a/s` runs from a to the field's end; a bare `a` is just a.
            (a, if step.is_some() { field.max } else { a })
        };
        let step = step.unwrap_or(1);
        let mut v = lo;
        while v <= hi {
            bits |= 1u64 << v;
            v += step;
        }
    }
    Ok(bits)
}

/// Read a schedule. The error is a sentence about what is wrong, for the
/// manifest validator to put after the trigger's position.
pub fn parse_cron(text: &str) -> Result<Cron, String> {
    let text = text.trim();
    let expanded = if text.starts_with('@') {
        macro_of(text).ok_or_else(|| {
            format!(
                "\"{text}\" is not a schedule shortcut (@hourly, @daily, @weekly, @monthly, @yearly)"
            )
        })?
    } else {
        text
    };
    let parts: Vec<&str> = expanded.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!(
            "a schedule has five fields (minute hour day-of-month month day-of-week), this one has {}",
            parts.len()
        ));
    }
    let mut bits = [0u64; 5];
    for (i, (field, part)) in FIELDS.iter().zip(&parts).enumerate() {
        bits[i] = parse_field(field, part)?;
    }
    // 7 is Sunday too.
    let mut weekdays = bits[4];
    if weekdays & (1 << 7) != 0 {
        weekdays = (weekdays & !(1 << 7)) | 1;
    }
    Ok(Cron {
        minutes: bits[0],
        hours: bits[1] as u32,
        days: bits[2] as u32,
        months: bits[3] as u16,
        weekdays: weekdays as u8,
        days_star: parts[2].starts_with('*'),
        weekdays_star: parts[4].starts_with('*'),
    })
}

/// (year, month 1-12, day 1-31) of a day count since 1970-01-01 — Howard
/// Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

impl Cron {
    fn day_ok(&self, day: i64) -> bool {
        let (_, month, dom) = civil_from_days(day);
        if self.months & (1 << month) == 0 {
            return false;
        }
        // 1970-01-01 was a Thursday (4).
        let dow = (day + 4).rem_euclid(7) as u32;
        let dom_ok = self.days & (1 << dom) != 0;
        let dow_ok = self.weekdays & (1 << dow) != 0;
        if self.days_star || self.weekdays_star {
            dom_ok && dow_ok
        } else {
            dom_ok || dow_ok
        }
    }

    fn time_ok(&self, minute_of_day: i64) -> bool {
        let (h, m) = (minute_of_day / 60, minute_of_day % 60);
        self.hours & (1 << h) != 0 && self.minutes & (1 << m) != 0
    }

    /// Whether the minute holding `at_ms` (UTC) is one this schedule names.
    pub fn matches(&self, at_ms: i64) -> bool {
        let minute = at_ms.div_euclid(MINUTE_MS);
        self.day_ok(minute.div_euclid(DAY_MINUTES)) && self.time_ok(minute.rem_euclid(DAY_MINUTES))
    }

    /// The latest minute this schedule names that is AFTER `after_ms` and at
    /// or before `now_ms`, as the epoch ms of its first instant — or None.
    /// What the scheduler fires: one run for the latest due minute, however
    /// many passed while nothing was watching.
    pub fn latest_due(&self, after_ms: i64, now_ms: i64) -> Option<i64> {
        let floor = after_ms.div_euclid(MINUTE_MS);
        let mut m = now_ms.div_euclid(MINUTE_MS);
        while m > floor {
            let day = m.div_euclid(DAY_MINUTES);
            if !self.day_ok(day) {
                m = day * DAY_MINUTES - 1;
                continue;
            }
            let of_day = m.rem_euclid(DAY_MINUTES);
            if self.hours & (1 << (of_day / 60)) == 0 {
                m -= of_day % 60 + 1;
                continue;
            }
            if self.time_ok(of_day) {
                return Some(m * MINUTE_MS);
            }
            m -= 1;
        }
        None
    }

    /// The first minute this schedule names strictly after `after_ms`, within
    /// `horizon_days` — None when it names none that soon (a 30th of
    /// February).
    pub fn next_after(&self, after_ms: i64, horizon_days: i64) -> Option<i64> {
        let mut m = after_ms.div_euclid(MINUTE_MS) + 1;
        let end = m + horizon_days * DAY_MINUTES;
        while m <= end {
            let day = m.div_euclid(DAY_MINUTES);
            if !self.day_ok(day) {
                m = (day + 1) * DAY_MINUTES;
                continue;
            }
            let of_day = m.rem_euclid(DAY_MINUTES);
            if self.hours & (1 << (of_day / 60)) == 0 {
                m += 60 - of_day % 60;
                continue;
            }
            if self.time_ok(of_day) {
                return Some(m * MINUTE_MS);
            }
            m += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-26T00:00:00Z, a Monday.
    const MON: i64 = 1_792_972_800_000;
    const H: i64 = 3_600_000;
    const M: i64 = 60_000;

    #[test]
    fn the_anchor_is_the_monday_it_claims() {
        let day = MON / (DAY_MINUTES * MINUTE_MS);
        assert_eq!(civil_from_days(day), (2026, 10, 26));
        assert_eq!((day + 4).rem_euclid(7), 1);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        // 2024-02-29 exists; the day after is March 1st.
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
    }

    #[test]
    fn reads_every_item_shape() {
        let c = parse_cron("*/15 9-17/4 1,15 * 1-5").unwrap();
        assert_eq!(c.minutes, 1 | 1 << 15 | 1 << 30 | 1 << 45);
        assert_eq!(c.hours, 1 << 9 | 1 << 13 | 1 << 17);
        assert_eq!(c.days, 1 << 1 | 1 << 15);
        assert_eq!(c.months, 0b1_1111_1111_1110);
        assert_eq!(c.weekdays, 0b11_1110);
        assert!(!c.days_star && !c.weekdays_star);
        let c = parse_cron("5/20 * * * 7").unwrap();
        assert_eq!(c.minutes, 1 << 5 | 1 << 25 | 1 << 45);
        assert_eq!(c.weekdays, 1, "7 is Sunday");
        assert_eq!(
            parse_cron("@daily").unwrap(),
            parse_cron("0 0 * * *").unwrap()
        );
        assert_eq!(
            parse_cron(" @hourly ").unwrap(),
            parse_cron("0 * * * *").unwrap()
        );
    }

    #[test]
    fn refuses_with_the_sentence_an_author_can_act_on() {
        let err = |s: &str| parse_cron(s).unwrap_err();
        assert!(err("* * * *").contains("five fields"), "{}", err("* * * *"));
        assert!(err("60 * * * *").contains("minute field's 60 is outside 0-59"));
        assert!(err("* 24 * * *").contains("hour field's 24 is outside 0-23"));
        assert!(err("* * 0 * *").contains("day of month field's 0 is outside 1-31"));
        assert!(err("* * * 13 *").contains("month field's 13"));
        assert!(err("* * * * 8").contains("day of week field's 8"));
        assert!(err("* * * JAN *").contains("\"JAN\" where a number belongs"));
        assert!(err("*/0 * * * *").contains("step \"0\""));
        assert!(err("5-1 * * * *").contains("runs backwards"));
        assert!(err("1,,2 * * * *").contains("empty list item"));
        assert!(err("@often").contains("not a schedule shortcut"));
    }

    #[test]
    fn the_latest_due_minute_is_the_last_one_named() {
        let nightly = parse_cron("0 3 * * *").unwrap();
        // At 05:10 on Monday, armed at midnight: 03:00 was due.
        assert_eq!(
            nightly.latest_due(MON, MON + 5 * H + 10 * M),
            Some(MON + 3 * H)
        );
        // Armed at 03:00 exactly: that minute is not after the arming.
        assert_eq!(nightly.latest_due(MON + 3 * H, MON + 5 * H), None);
        // Two days missed: one run, for the latest.
        assert_eq!(
            nightly.latest_due(MON, MON + 2 * 24 * H + 4 * H),
            Some(MON + 2 * 24 * H + 3 * H)
        );
        let every = parse_cron("* * * * *").unwrap();
        assert_eq!(every.latest_due(MON, MON + 90_500), Some(MON + M));
        assert_eq!(every.latest_due(MON + 30_000, MON + 59_000), None);
    }

    #[test]
    fn the_next_minute_honours_the_day_rule() {
        // Both day fields restricted: the 1st OR a Monday.
        let either = parse_cron("0 9 1 * 1").unwrap();
        assert_eq!(either.next_after(MON, 40), Some(MON + 9 * H));
        // After Monday 09:00 the next is the 1st of November (a Sunday).
        let nov1 = MON + 6 * 24 * H;
        assert_eq!(either.next_after(MON + 9 * H, 40), Some(nov1 + 9 * H));
        // A day field that starts with `*` makes it AND: every day of the
        // month, AND a Monday — so Mondays only, never Sunday the 1st.
        let both = parse_cron("0 9 */1 * 1").unwrap();
        assert!(both.matches(MON + 9 * H));
        assert!(!both.matches(nov1 + 9 * H));
        assert!(either.matches(nov1 + 9 * H));
        let feb30 = parse_cron("0 0 30 2 *").unwrap();
        assert_eq!(feb30.next_after(MON, 400), None);
        assert!(
            parse_cron("30 * * * *")
                .unwrap()
                .matches(MON + 4 * H + 30 * M + 59_999)
        );
    }
}
