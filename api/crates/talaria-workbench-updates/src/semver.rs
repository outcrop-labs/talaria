// Release numbers and the `engines` ranges npm packages declare — the subset
// the channel needs to answer one question: does this bun satisfy this Oh My
// Pi's `engines.bun`? Plain `x.y.z` releases only (a prerelease is never a
// candidate), and the comparators npm ranges are written with: `>=`, `>`,
// `<=`, `<`, `=`, a bare or partial version, `^`, `~`, `*`/`x`, a hyphen
// range, AND by spaces and OR by `||`. A range it cannot read is `None`, and
// the caller treats that as unsatisfied: it fails closed.

/// A plain release number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ver(pub u64, pub u64, pub u64);

/// `x.y.z`, digits only — the only shape a release may have here.
pub fn parse(v: &str) -> Option<Ver> {
    if !talaria_workbench_harnesses::plain_version(v) {
        return None;
    }
    let mut it = v.split('.').map(|p| p.parse::<u64>().ok());
    Some(Ver(it.next()??, it.next()??, it.next()??))
}

/// Order two plain releases (an unreadable one sorts first).
pub fn cmp(a: &str, b: &str) -> std::cmp::Ordering {
    parse(a).cmp(&parse(b))
}

/// `a` is a newer plain release than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    matches!((parse(a), parse(b)), (Some(x), Some(y)) if x > y)
}

/// A partial version as written in a range: `1`, `1.4`, `1.4.4`, with `x`
/// or `*` wildcards. The parts given, in order.
fn partial(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches('v');
    if s.is_empty() || s == "*" || s.eq_ignore_ascii_case("x") {
        return Some(vec![]);
    }
    let mut out = Vec::new();
    for p in s.split('.') {
        if p == "*" || p.eq_ignore_ascii_case("x") {
            break;
        }
        if p.is_empty() || p.len() > 9 || !p.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        out.push(p.parse().ok()?);
    }
    (out.len() <= 3).then_some(out)
}

fn floor(p: &[u64]) -> Ver {
    Ver(
        p.first().copied().unwrap_or(0),
        p.get(1).copied().unwrap_or(0),
        p.get(2).copied().unwrap_or(0),
    )
}

/// The first version past a partial one (`1.4` → `1.5.0`, `1` → `2.0.0`);
/// `None` for a full or empty one (no upper bound from it).
fn past(p: &[u64]) -> Option<Ver> {
    match p.len() {
        1 => Some(Ver(p[0] + 1, 0, 0)),
        2 => Some(Ver(p[0], p[1] + 1, 0)),
        _ => None,
    }
}

/// A bound: `(lower, inclusive)` and `(upper, inclusive)`.
type Interval = (Option<(Ver, bool)>, Option<(Ver, bool)>);

fn comparator(tok: &str) -> Option<Interval> {
    let (op, rest) = ["<=", ">=", "<", ">", "=", "^", "~"]
        .iter()
        .find_map(|op| tok.strip_prefix(op).map(|r| (*op, r)))
        .unwrap_or(("", tok));
    let p = partial(rest)?;
    let lo = floor(&p);
    Some(match op {
        ">=" => (Some((lo, true)), None),
        ">" => match past(&p) {
            Some(up) if !p.is_empty() => (Some((up, true)), None),
            _ if p.is_empty() => (Some((Ver(u64::MAX, 0, 0), true)), None),
            _ => (Some((lo, false)), None),
        },
        "<" => (None, Some((lo, false))),
        "<=" => match past(&p) {
            Some(up) => (None, Some((up, false))),
            None if p.is_empty() => (None, None),
            None => (None, Some((lo, true))),
        },
        "^" => {
            let up = match p.as_slice() {
                [] => None,
                [0] => Some(Ver(1, 0, 0)),
                [0, 0] => Some(Ver(0, 1, 0)),
                [0, 0, z] => Some(Ver(0, 0, z + 1)),
                [0, y, ..] => Some(Ver(0, y + 1, 0)),
                [x, ..] => Some(Ver(x + 1, 0, 0)),
            };
            (Some((lo, true)), up.map(|u| (u, false)))
        }
        "~" => {
            let up = match p.as_slice() {
                [] => None,
                [x] => Some(Ver(x + 1, 0, 0)),
                [x, y, ..] => Some(Ver(*x, y + 1, 0)),
            };
            (Some((lo, true)), up.map(|u| (u, false)))
        }
        _ => match past(&p) {
            // `1.4` is `>=1.4.0 <1.5.0`; `*` is anything.
            Some(up) => (Some((lo, true)), Some((up, false))),
            None if p.is_empty() => (None, None),
            None => (Some((lo, true)), Some((lo, true))),
        },
    })
}

fn within(v: Ver, (lo, hi): &Interval) -> bool {
    let above = match lo {
        None => true,
        Some((b, true)) => v >= *b,
        Some((b, false)) => v > *b,
    };
    let below = match hi {
        None => true,
        Some((b, true)) => v <= *b,
        Some((b, false)) => v < *b,
    };
    above && below
}

/// Does plain release `v` satisfy npm range `range`? `None` when either is
/// unreadable (a prerelease, a tag, a range with a syntax this subset does
/// not read) — which a caller treats as no.
pub fn satisfies(v: &str, range: &str) -> Option<bool> {
    let v = parse(v)?;
    let range = range.trim();
    if range.len() > 200 {
        return None;
    }
    let mut any = false;
    for set in range.split("||") {
        let set = set.trim();
        // A hyphen range: `A - B` is `>=A <=B`.
        let toks: Vec<String> = if let Some((a, b)) = set.split_once(" - ") {
            vec![format!(">={}", a.trim()), format!("<={}", b.trim())]
        } else {
            // `>= 1.4.4` (an operator spaced from its version) is one token.
            let mut toks: Vec<String> = Vec::new();
            for t in set.split_whitespace() {
                match toks.last_mut() {
                    Some(last)
                        if ["<=", ">=", "<", ">", "=", "^", "~"].contains(&last.as_str()) =>
                    {
                        last.push_str(t)
                    }
                    _ => toks.push(t.to_string()),
                }
            }
            toks
        };
        let mut ok = true;
        for t in &toks {
            let iv = comparator(t)?;
            ok &= within(v, &iv);
        }
        any |= ok;
    }
    Some(any)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_releases_order_numerically() {
        assert!(newer("1.4.10", "1.4.9"));
        assert!(newer("18.16.0", "18.9.3"));
        assert!(!newer("1.4.4", "1.4.4"));
        assert!(!newer("latest", "1.0.0") && !newer("1.0.0-rc.1", "0.9.0"));
    }

    #[test]
    fn the_ranges_omp_has_declared_read_as_npm_reads_them() {
        // What @oh-my-pi/pi-coding-agent's engines.bun has said: 18.13.2 and
        // before, then 18.13.3 on.
        assert_eq!(satisfies("1.4.0", ">=1.3.14"), Some(true));
        assert_eq!(satisfies("1.4.0", ">=1.4.4"), Some(false));
        assert_eq!(satisfies("1.4.4", ">=1.4.4"), Some(true));
        assert_eq!(satisfies("1.3.14", ">= 1.4.4"), Some(false));
        for (v, r, want) in [
            ("1.4.9", "^1.4.4", true),
            ("2.0.0", "^1.4.4", false),
            ("0.3.9", "^0.3.1", true),
            ("0.4.0", "^0.3.1", false),
            ("1.4.9", "~1.4.4", true),
            ("1.5.0", "~1.4.4", false),
            ("1.4.7", "1.4", true),
            ("1.5.0", "1.4.x", false),
            ("1.4.4", "=1.4.4", true),
            ("1.4.5", "1.4.4", false),
            ("1.4.4", ">1.4.3 <1.5.0", true),
            ("1.5.0", ">1.4.3 <1.5.0", false),
            ("1.2.0", "<1.3 || >=1.4.4", true),
            ("1.3.5", "<1.3 || >=1.4.4", false),
            ("1.4.6", "1.4.4 - 1.4.6", true),
            ("1.4.7", "1.4.4 - 1.4.6", false),
            ("9.9.9", "*", true),
            ("1.5.0", "<=1.4", false),
            ("1.4.9", "<=1.4", true),
            ("1.4.5", ">1.4", false),
            ("1.5.0", ">1.4", true),
        ] {
            assert_eq!(satisfies(v, r), Some(want), "{v} in {r}");
        }
    }

    #[test]
    fn an_unreadable_range_or_version_is_none_never_a_yes() {
        assert_eq!(satisfies("1.4.4", ">=1.4.4-canary.1"), None);
        assert_eq!(satisfies("1.4.4", "workspace:*"), None);
        assert_eq!(satisfies("1.4.4", "latest"), None);
        assert_eq!(satisfies("v1.4.4", ">=1.0.0"), None);
        assert_eq!(satisfies("1.4.4", &"1".repeat(300)), None);
    }
}
