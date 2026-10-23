// An app's manifest, `talaria.json` — the one validator. Install and update
// (talaria-apps) run it on the fetched tree before anything lands in apps/;
// discovery (talaria-users) runs it on every directory under apps/, so an app
// whose manifest fails is listed as broken (and stays removable) instead of
// vanishing. The TS host holds manifests to the same rules
// (ui/src/server/app-build/manifest.ts); the shared fixture
// api/tests/it/fixtures/app-manifest.json pins the two, the host's SDK
// version included, and docs/sdk/talaria.schema.json publishes the shape.
//
// A leaf: serde_json only, so talaria-users can depend on it without a cycle.

use serde_json::{Map, Value};

/// The version of `@talaria/sdk` this host provides. An app's manifest may
/// name the range it was written against (`"sdk": "^1.0"`); a host outside
/// that range refuses to install it and lists an installed one as broken.
/// A minor bump adds to the SDK, a major bump breaks it. The TS twin is
/// `SDK_VERSION` in ui/src/server/app-build/manifest.ts, and both are pinned
/// to the shared fixture's `sdkVersion`.
pub const SDK_VERSION: &str = "1.0.0";

/// Apps' slug rule: lowercase letters, digits, dashes; a letter-or-digit
/// head; ≤64 chars total.
pub fn slug_ok(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 64 {
        return false;
    }
    let head = b[0].is_ascii_lowercase() || b[0].is_ascii_digit();
    let rest = b[1..]
        .iter()
        .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    head && rest
}

/// Slugs no NEW app may take: the client router's config keys. An app's
/// slug is a path segment under /x, and sv-router reads a `layout`, `hooks`
/// or `meta` key in its route table as a path segment too — one beside the
/// slug segment outranked '/:app', so /x/hooks matched the key, found no
/// route, and rendered a blank page. The route table keeps those keys above
/// /x (ui/src/route-table.ts), which is what lets an app that already has
/// one of these slugs open; install refuses them so no new app leans on it.
/// Discovery, enablement and uninstall still take them (`slug_ok`), so an
/// app installed before the refusal stays manageable. The list is the shared
/// fixture's `routes.reservedSlugs` (api/tests/it/fixtures/app-grants.json),
/// which the TS route-table test and the CLI's `talaria app new` read too.
pub const RESERVED_APP_SLUGS: &[&str] = &["layout", "hooks", "meta"];

/// Whether a new app may be installed under `s`: the slug rule, and not one
/// of `RESERVED_APP_SLUGS`. The error is the sentence the admin reads.
pub fn slug_installable(s: &str) -> Result<(), String> {
    if !slug_ok(s) {
        return Err(format!(
            "\"{s}\" is not a usable app slug (lowercase letters, digits, dashes)"
        ));
    }
    if RESERVED_APP_SLUGS.contains(&s) {
        return Err(format!(
            "\"{s}\" is reserved: the client router uses that word in its route table — install the app under another slug"
        ));
    }
    Ok(())
}

/// The surfaces a manifest may declare, in the order the host lists them.
pub const SURFACES: [&str; 3] = ["work", "manage", "settings"];

/// Every key the manifest gives a meaning to. Any other top-level key is
/// ignored, so a manifest written for a newer host still reads here when its
/// `sdk` range allows. `harnesses` and `triggers` are reserved: their shape is
/// checked, and nothing reads them yet.
pub const KNOWN_KEYS: [&str; 10] = [
    "$schema",
    "name",
    "slug",
    "icon",
    "version",
    "description",
    "surfaces",
    "sdk",
    "harnesses",
    "triggers",
];

/// Length caps, in Unicode code points (the TS side counts the same way).
pub const NAME_MAX: usize = 120;
pub const SURFACE_TITLE_MAX: usize = 120;
pub const ICON_MAX: usize = 16;
pub const VERSION_MAX: usize = 64;
pub const DESCRIPTION_MAX: usize = 2000;

/// A manifest that passed. Absent optional fields are their defaults, as the
/// listings serve them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    /// The slug the app asks to be installed as. Install uses it, and
    /// discovery holds the directory name to it.
    pub slug: Option<String>,
    pub icon: String,
    pub description: String,
    pub version: String,
    pub work: Option<String>,
    pub manage: Option<String>,
    pub settings: Option<String>,
    /// The `@talaria/sdk` range the app was written against.
    pub sdk: Option<String>,
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn optional_string(
    obj: &Map<String, Value>,
    key: &str,
    max: usize,
) -> Result<Option<String>, String> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if chars(s) > max => {
            Err(format!("\"{key}\" is longer than {max} characters"))
        }
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

/// Read and validate a manifest file's bytes against the host's SDK version.
/// The bytes must be UTF-8 JSON with no byte-order mark, as the TS host reads
/// them (a fatal decoder that keeps the BOM).
pub fn parse_manifest(bytes: &[u8], host_sdk: &str) -> Result<Manifest, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "talaria.json is not UTF-8".to_string())?;
    let value: Value =
        serde_json::from_str(text).map_err(|e| format!("talaria.json is not valid JSON: {e}"))?;
    validate_manifest(&value, host_sdk)
}

/// The rules, on a parsed value. Each error is the sentence an admin (or the
/// author) reads: what is wrong and where.
pub fn validate_manifest(value: &Value, host_sdk: &str) -> Result<Manifest, String> {
    let Some(obj) = value.as_object() else {
        return Err("talaria.json must be a JSON object".into());
    };
    let name = match obj.get("name") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        _ => return Err("talaria.json needs a \"name\": a non-empty string".into()),
    };
    if chars(&name) > NAME_MAX {
        return Err(format!("\"name\" is longer than {NAME_MAX} characters"));
    }
    let slug = match obj.get("slug") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            slug_installable(s).map_err(|e| format!("\"slug\": {e}"))?;
            Some(s.clone())
        }
        Some(_) => return Err("\"slug\" must be a string".into()),
    };
    let icon = optional_string(obj, "icon", ICON_MAX)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "⬡".into());
    let description = optional_string(obj, "description", DESCRIPTION_MAX)?.unwrap_or_default();
    let version = optional_string(obj, "version", VERSION_MAX)?
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0.0.0".into());

    let mut surfaces: [Option<String>; 3] = [None, None, None];
    match obj.get("surfaces") {
        None | Some(Value::Null) => {}
        Some(Value::Object(s)) => {
            for (key, v) in s {
                let Some(i) = SURFACES.iter().position(|k| k == key) else {
                    return Err(format!(
                        "\"surfaces\" names an unknown surface \"{key}\" (expected work, manage or settings)"
                    ));
                };
                match v {
                    Value::String(t) if !t.trim().is_empty() => {
                        if chars(t) > SURFACE_TITLE_MAX {
                            return Err(format!(
                                "\"surfaces.{key}\" is longer than {SURFACE_TITLE_MAX} characters"
                            ));
                        }
                        surfaces[i] = Some(t.clone());
                    }
                    _ => {
                        return Err(format!(
                            "\"surfaces.{key}\" must be a non-empty string (the surface's title)"
                        ));
                    }
                }
            }
        }
        Some(_) => return Err("\"surfaces\" must be an object".into()),
    }

    let sdk = match obj.get("sdk") {
        None | Some(Value::Null) => None,
        Some(Value::String(range)) => {
            let ok = satisfies(range, host_sdk)
                .map_err(|e| format!("\"sdk\" is not a version range: {e}"))?;
            if !ok {
                return Err(format!(
                    "this app needs @talaria/sdk {range}; this host provides {host_sdk}"
                ));
            }
            Some(range.clone())
        }
        Some(_) => {
            return Err("\"sdk\" must be a string (a version range such as \"^1.0\")".into());
        }
    };

    match obj.get("harnesses") {
        None | Some(Value::Null) | Some(Value::Array(_)) | Some(Value::Object(_)) => {}
        Some(_) => {
            return Err(
                "\"harnesses\" must be an array or an object (reserved for the harness registry)"
                    .into(),
            );
        }
    }
    match obj.get("triggers") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for (i, t) in items.iter().enumerate() {
                let Some(t) = t.as_object() else {
                    return Err(format!("\"triggers[{i}]\" must be an object"));
                };
                match t.get("id") {
                    Some(Value::String(id)) if slug_ok(id) => {}
                    _ => {
                        return Err(format!(
                            "\"triggers[{i}].id\" must be a lowercase id (letters, digits, dashes)"
                        ));
                    }
                }
                let cron = t
                    .get("cron")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                let event = t
                    .get("event")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty());
                if cron == event {
                    return Err(format!(
                        "\"triggers[{i}]\" needs exactly one of \"cron\" or \"event\" (a non-empty string)"
                    ));
                }
            }
        }
        Some(_) => return Err("\"triggers\" must be an array".into()),
    }

    let [work, manage, settings] = surfaces;
    Ok(Manifest {
        name,
        slug,
        icon,
        description,
        version,
        work,
        manage,
        settings,
        sdk,
    })
}

// ── Version ranges ───────────────────────────────────────────────────────────
//
// The npm range grammar an app author already knows, without pre-release
// tags or hyphen ranges: `1.2.3`, `1.2`, `1.x`, `*`, `^1.2`, `~1.2.3`,
// `>=1.0 <2`, `^1 || ^2`. The TS twin (`satisfiesRange`) follows the same
// rules, pinned by the shared fixture's `ranges`.

/// A release version, `major.minor.patch`.
pub type Version = (u64, u64, u64);

/// `1.2.3` → (1, 2, 3). Exactly three numeric parts.
pub fn parse_version(s: &str) -> Result<Version, String> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.len() != 3 {
        return Err(format!("\"{s}\" is not a version (major.minor.patch)"));
    }
    let n = |p: &str| -> Result<u64, String> {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("\"{s}\" is not a version (major.minor.patch)"));
        }
        p.parse::<u64>()
            .map_err(|_| format!("\"{s}\" is out of range"))
    };
    Ok((n(parts[0])?, n(parts[1])?, n(parts[2])?))
}

#[derive(Debug, Clone, Copy)]
enum Bound {
    Ge(Version),
    Gt(Version),
    Lt(Version),
    Le(Version),
}

impl Bound {
    fn admits(self, v: Version) -> bool {
        match self {
            Bound::Ge(b) => v >= b,
            Bound::Gt(b) => v > b,
            Bound::Lt(b) => v < b,
            Bound::Le(b) => v <= b,
        }
    }
}

/// A partial version: `1`, `1.2`, `1.2.3`, with `x`/`X`/`*` for a part that
/// is left open (and every part after it).
type Partial = (Option<u64>, Option<u64>, Option<u64>);

fn parse_partial(s: &str) -> Result<Partial, String> {
    if s.contains('-') || s.contains('+') {
        return Err(format!(
            "\"{s}\": pre-release and build tags are not supported"
        ));
    }
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err(format!("\"{s}\" is not a version"));
    }
    let mut out: [Option<u64>; 3] = [None, None, None];
    let mut open = false;
    for (i, p) in parts.iter().enumerate() {
        if matches!(*p, "x" | "X" | "*") {
            open = true;
            continue;
        }
        if open {
            return Err(format!("\"{s}\": a number cannot follow a wildcard"));
        }
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("\"{s}\" is not a version"));
        }
        out[i] = Some(
            p.parse::<u64>()
                .map_err(|_| format!("\"{s}\" is out of range"))?,
        );
    }
    Ok((out[0], out[1], out[2]))
}

/// The bounds one comparator (`^1.2`, `>=1`, `1.x`, …) stands for.
fn comparator(op: &str, partial: &str) -> Result<Vec<Bound>, String> {
    let (maj, min, pat) = parse_partial(partial)?;
    let Some(maj) = maj else {
        // `*`, `x`: any version — only as a bare or >= / ^ / ~ / = range.
        return match op {
            "" | "=" | ">=" | "^" | "~" => Ok(vec![]),
            _ => Err(format!("\"{op}{partial}\" admits no version")),
        };
    };
    let lower = (maj, min.unwrap_or(0), pat.unwrap_or(0));
    let next_major = (maj + 1, 0, 0);
    let next_minor = |m: u64| (maj, m + 1, 0);
    Ok(match (op, min, pat) {
        ("" | "=", None, _) => vec![Bound::Ge(lower), Bound::Lt(next_major)],
        ("" | "=", Some(m), None) => vec![Bound::Ge(lower), Bound::Lt(next_minor(m))],
        ("" | "=", Some(_), Some(_)) => vec![Bound::Ge(lower), Bound::Le(lower)],
        ("^", None, _) => vec![Bound::Ge(lower), Bound::Lt(next_major)],
        ("^", Some(m), None) => {
            if maj > 0 {
                vec![Bound::Ge(lower), Bound::Lt(next_major)]
            } else {
                vec![Bound::Ge(lower), Bound::Lt(next_minor(m))]
            }
        }
        ("^", Some(m), Some(p)) => {
            if maj > 0 {
                vec![Bound::Ge(lower), Bound::Lt(next_major)]
            } else if m > 0 {
                vec![Bound::Ge(lower), Bound::Lt(next_minor(m))]
            } else {
                vec![Bound::Ge(lower), Bound::Lt((0, 0, p + 1))]
            }
        }
        ("~", None, _) => vec![Bound::Ge(lower), Bound::Lt(next_major)],
        ("~", Some(m), _) => vec![Bound::Ge(lower), Bound::Lt(next_minor(m))],
        (">=", _, _) => vec![Bound::Ge(lower)],
        (">", None, _) => vec![Bound::Ge(next_major)],
        (">", Some(m), None) => vec![Bound::Ge(next_minor(m))],
        (">", Some(_), Some(_)) => vec![Bound::Gt(lower)],
        ("<", _, _) => vec![Bound::Lt(lower)],
        ("<=", None, _) => vec![Bound::Lt(next_major)],
        ("<=", Some(m), None) => vec![Bound::Lt(next_minor(m))],
        ("<=", Some(_), Some(_)) => vec![Bound::Le(lower)],
        _ => return Err(format!("\"{op}\" is not a range operator")),
    })
}

const OPS: [&str; 7] = [">=", "<=", ">", "<", "=", "^", "~"];

fn split_op(token: &str) -> (&str, &str) {
    for op in OPS {
        if let Some(rest) = token.strip_prefix(op) {
            return (op, rest);
        }
    }
    ("", token)
}

/// One `||` alternative: comparators separated by whitespace, all of which
/// must hold. An operator may stand apart from its version (`>= 1.2`).
fn comparator_set(set: &str) -> Result<Vec<Bound>, String> {
    let tokens: Vec<&str> = set.split_whitespace().collect();
    if tokens.is_empty() {
        return Ok(vec![]);
    }
    let mut bounds = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i] == "-" {
            return Err("hyphen ranges (a - b) are not supported; write >=a <=b".into());
        }
        let (op, rest) = split_op(tokens[i]);
        let (op, partial) = if rest.is_empty() && !op.is_empty() {
            i += 1;
            let Some(next) = tokens.get(i) else {
                return Err(format!("\"{op}\" needs a version after it"));
            };
            (op, *next)
        } else {
            (op, rest)
        };
        if partial.is_empty() {
            return Err(format!("\"{}\" is not a version", tokens[i]));
        }
        if !split_op(partial).0.is_empty() {
            return Err(format!("\"{}\" has two operators", tokens[i]));
        }
        bounds.extend(comparator(op, partial)?);
        i += 1;
    }
    Ok(bounds)
}

/// Whether `version` (`major.minor.patch`) is in `range`. An error names what
/// is wrong with the range.
pub fn satisfies(range: &str, version: &str) -> Result<bool, String> {
    let v = parse_version(version)?;
    let range = range.trim();
    if range.len() > 256 {
        return Err("the range is longer than 256 characters".into());
    }
    let mut any = false;
    for set in range.split("||") {
        let bounds = comparator_set(set)?;
        if bounds.iter().all(|b| b.admits(v)) {
            any = true;
        }
    }
    Ok(any)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shared fixture the TS host asserts too
    /// (ui/src/server/app-build/manifest.test.ts).
    const FIXTURE: &str = include_str!("../../../tests/it/fixtures/app-manifest.json");

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("app-manifest.json parses")
    }

    #[test]
    fn the_hosts_sdk_version_is_the_fixtures() {
        assert_eq!(fixture()["sdkVersion"], SDK_VERSION);
        parse_version(SDK_VERSION).expect("SDK_VERSION is a version");
    }

    #[test]
    fn every_fixture_manifest_is_judged_as_the_ts_host_judges_it() {
        let f = fixture();
        let host = f["sdkVersion"].as_str().unwrap();
        for case in f["manifests"].as_array().unwrap() {
            let about = case["about"].as_str().unwrap();
            let got = validate_manifest(&case["manifest"], host);
            match case.get("error").and_then(Value::as_str) {
                Some(want) => {
                    let err = got.expect_err(about);
                    assert!(err.contains(want), "{about}: {err:?} does not say {want:?}");
                }
                None => {
                    let m = got.unwrap_or_else(|e| panic!("{about}: {e}"));
                    if let Some(want) = case.get("parsed") {
                        assert_eq!(want["name"], m.name, "{about}");
                        assert_eq!(want["icon"], m.icon, "{about}");
                        assert_eq!(want["version"], m.version, "{about}");
                        assert_eq!(
                            want.get("slug").and_then(Value::as_str),
                            m.slug.as_deref(),
                            "{about}"
                        );
                        let surfaces: Vec<&str> = SURFACES
                            .iter()
                            .zip([&m.work, &m.manage, &m.settings])
                            .filter(|(_, v)| v.is_some())
                            .map(|(k, _)| *k)
                            .collect();
                        let want_surfaces: Vec<&str> = want["surfaces"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|v| v.as_str().unwrap())
                            .collect();
                        assert_eq!(surfaces, want_surfaces, "{about}");
                    }
                }
            }
        }
    }

    #[test]
    fn every_fixture_range_is_read_as_the_ts_host_reads_it() {
        let f = fixture();
        for case in f["ranges"].as_array().unwrap() {
            let range = case["range"].as_str().unwrap();
            let version = case["version"].as_str().unwrap();
            let got = satisfies(range, version);
            match case.get("error").and_then(Value::as_str) {
                Some(want) => {
                    let err = got.expect_err(range);
                    assert!(err.contains(want), "{range}: {err:?} does not say {want:?}");
                }
                None => assert_eq!(
                    got.unwrap_or_else(|e| panic!("{range}: {e}")),
                    case["satisfied"].as_bool().unwrap(),
                    "{range} @ {version}"
                ),
            }
        }
    }

    #[test]
    fn the_file_reader_refuses_what_the_ts_decoder_refuses() {
        assert_eq!(
            parse_manifest(b"{\"name\":\"Caf\xe9\"}", SDK_VERSION).unwrap_err(),
            "talaria.json is not UTF-8"
        );
        let bom = parse_manifest(b"\xef\xbb\xbf{\"name\":\"Bom\"}", SDK_VERSION).unwrap_err();
        assert!(bom.starts_with("talaria.json is not valid JSON"), "{bom}");
        assert!(
            parse_manifest(b"{\"name\":", SDK_VERSION)
                .unwrap_err()
                .starts_with("talaria.json is not valid JSON")
        );
        assert_eq!(
            parse_manifest(
                br#"{"name":"Plain","surfaces":{"work":"Plain"}}"#,
                SDK_VERSION
            )
            .unwrap()
            .work
            .as_deref(),
            Some("Plain")
        );
    }

    #[test]
    fn a_declared_slug_is_held_to_the_install_rule() {
        let err =
            validate_manifest(&json!({"name": "Hooks", "slug": "hooks"}), SDK_VERSION).unwrap_err();
        assert!(err.contains("is reserved"), "{err}");
        let err =
            validate_manifest(&json!({"name": "X", "slug": "Bad Slug"}), SDK_VERSION).unwrap_err();
        assert!(err.contains("not a usable app slug"), "{err}");
    }

    #[test]
    fn slug_rules() {
        assert!(slug_ok("crm"));
        assert!(slug_ok("a1-b2"));
        assert!(!slug_ok("-crm"));
        assert!(!slug_ok(".crm.next"));
        assert!(!slug_ok(&"a".repeat(65)));
        assert!(slug_installable("crm").is_ok());
        for s in RESERVED_APP_SLUGS {
            assert!(slug_installable(s).is_err());
        }
    }
}
