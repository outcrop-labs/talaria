// Catalog shaping. DB-free on purpose: every rule here has a unit test, and
// the chat relay resolves model ids against this same shaping.

/// One `llm_endpoints` row, exactly the columns the catalog reads.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EndpointModels {
    pub name: String,
    /// jsonb array column; sqlx decodes with the `json` feature.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayModel {
    pub id: String,
    pub endpoints: Vec<String>,
    /// True for `<endpoint>/<model>` pins. Bare model ids may themselves
    /// contain "/" (OpenRouter, HF-style names) — only this flag tells the
    /// two apart.
    pub qualified: bool,
}

/// Every model, spelled one way: `<endpoint>/<model>`. A bare id appears only
/// when more than one endpoint serves it, carrying every endpoint in
/// first-seen order (local endpoints first, then name asc — the SQL's order).
pub fn catalog_of(eps: &[EndpointModels]) -> Vec<GatewayModel> {
    // Vec-of-pairs instead of a HashMap: the endpoints list on a pooled bare
    // id must stay in first-seen order for `owned_by: talaria:<eps joined>`,
    // and Rust's HashMap has no iteration order to lean on.
    let mut bare: Vec<(String, Vec<String>)> = Vec::new();
    let mut out: Vec<GatewayModel> = Vec::new();
    for ep in eps {
        for m in &ep.models {
            out.push(GatewayModel {
                id: format!("{}/{}", ep.name, m),
                endpoints: vec![ep.name.clone()],
                qualified: true,
            });
            match bare.iter_mut().find(|(id, _)| id == m) {
                Some((_, names)) => names.push(ep.name.clone()),
                None => bare.push((m.clone(), vec![ep.name.clone()])),
            }
        }
    }
    for (id, endpoints) in bare {
        if endpoints.len() > 1 {
            out.push(GatewayModel {
                id,
                endpoints,
                qualified: false,
            });
        }
    }
    out.sort_by(|a, b| locale_cmp(&a.id, &b.id));
    out
}

// The catalog sorts with ICU root collation (`localeCompare`), and it is
// NOT byte order: this dev database catalogs "Z.ai" (uppercase) alongside
// "openrouter", and ICU puts Z after o while bytes put it before. The sort
// order is a stable contract, so here is ICU's collation for the grammar
// model ids actually use, derived from the probes in the tests below (each
// asserted against real localeCompare output).
//
// ICU compares in LEVELS, not per character: every primary weight first, case
// (tertiary) only afterwards. Primary weights for this grammar:
//
//   '_' < '-' < '.' < '/' < digits (by value, NO numeric folding: 2 > 10)
//   < letters (case-folded; a shorter folded string sorts first)
//
// Only when the folded forms match exactly does case decide, lowercase first
// ('a' < 'A'). This is why "GPT-4o" < "gpt-4o-mini" — the folded prefix ends
// the primary comparison before the case difference is ever reached.
//
// Chars outside this grammar (exotic punctuation, non-ASCII) get class 6 and
// sort after letters — an approximation, recorded in docs/RUST-MIGRATION.md.
// Ids are endpoint names operators pick plus provider model names; both are
// ASCII of this grammar in every catalog Talaria has shipped.
fn collation_class(c: char) -> u8 {
    match c {
        '_' => 0,
        '-' => 1,
        '.' => 2,
        '/' => 3,
        '0'..='9' => 4,
        'a'..='z' | 'A'..='Z' => 5,
        _ => 6,
    }
}

fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let fold = |c: char| c.to_ascii_lowercase();
    // Level 1 — primary weights, case-blind; a folded prefix sorts first.
    let mut ai = a.chars();
    let mut bi = b.chars();
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => break, // folded-equal: fall through to case
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let key_x = (collation_class(x), fold(x));
                let key_y = (collation_class(y), fold(y));
                if key_x != key_y {
                    return key_x.cmp(&key_y);
                }
            }
        }
    }
    // Level 3 — case, at the first differing position: lowercase first.
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            return if x.is_ascii_uppercase() {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(name: &str, models: &[&str]) -> EndpointModels {
        EndpointModels {
            name: name.into(),
            models: models.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn qualified_ids_and_pooled_bare_ids() {
        let catalog = catalog_of(&[
            ep("anthropic", &["claude-a", "claude-b"]),
            ep("openrouter", &["claude-a", "o-big"]),
        ]);

        let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
        // Sorted; "claude-a" appears bare (pooled) alongside BOTH qualifieds —
        // openrouter serves it too, so it emits its own pin.
        assert_eq!(
            ids,
            [
                "anthropic/claude-a",
                "anthropic/claude-b",
                "claude-a",
                "openrouter/claude-a",
                "openrouter/o-big"
            ]
        );

        let pooled = catalog.iter().find(|m| !m.qualified).unwrap();
        // First-seen order — anthropic row came first in the SQL (name asc).
        assert_eq!(pooled.endpoints, ["anthropic", "openrouter"]);

        let qualified = catalog.iter().find(|m| m.id == "openrouter/o-big").unwrap();
        assert!(qualified.qualified);
        assert_eq!(qualified.endpoints, ["openrouter"]);
    }

    #[test]
    fn bare_ids_may_contain_slashes() {
        // An OpenRouter-style bare id with slashes must NOT be mistaken for a
        // pin, and pooling keys on the WHOLE string.
        let catalog = catalog_of(&[
            ep("openrouter", &["meta/llama-4"]),
            ep("hf", &["meta/llama-4"]),
        ]);
        let bare = catalog.iter().find(|m| m.id == "meta/llama-4").unwrap();
        assert!(!bare.qualified);
        assert_eq!(bare.endpoints, ["openrouter", "hf"]);
    }

    #[test]
    fn single_endpoint_models_stay_qualified_only() {
        let catalog = catalog_of(&[ep("only", &["solo"])]);
        assert_eq!(catalog.len(), 1);
        assert!(catalog[0].qualified);
    }

    #[test]
    fn empty_catalog_and_empty_models() {
        assert!(catalog_of(&[]).is_empty());
        assert!(catalog_of(&[ep("hollow", &[])]).is_empty());
    }

    #[test]
    fn duplicate_listing_within_one_endpoint_counts_as_pooled() {
        // A row listing a model twice makes endpoints.len() > 1 on its own —
        // pooling keys on occurrences, not distinct endpoints. Odd, but that
        // is the rule.
        let catalog = catalog_of(&[ep("echo", &["m", "m"])]);
        let bare = catalog.iter().find(|m| !m.qualified).unwrap();
        assert_eq!(bare.endpoints, ["echo", "echo"]);
    }

    /// Every expectation below is real `localeCompare` output (ICU root) —
    /// the table above was derived from these pairs and this
    /// test is what keeps it honest.
    #[test]
    fn collation_matches_localecompare_pair_probes() {
        fn less(a: &str, b: &str) {
            assert_eq!(
                locale_cmp(a, b),
                std::cmp::Ordering::Less,
                "{a} should sort before {b}"
            );
        }
        less("a_", "a-"); // '_' lowest
        less("a-", "a.");
        less("a.", "a/"); // punctuation rungs, probed one by one
        less("a-b", "ab"); // all punctuation under letters
        less("a1", "aa"); // digits under letters
        less("a/1", "a1"); // '/' under digits
        less("meta-llama", "meta/llama-4"); // '-' under '/'
        less("model-10", "model-2"); // NO numeric folding: "1" < "2"
        less("gpt-4.1", "gpt-4o"); // '.' under letters
        less("claude-a", "claude-A"); // case tie: lowercase first
        less("GPT-4o", "gpt-4o-mini"); // case-folded prefix, shorter first
        less("o3", "o3-mini");
    }

    /// The full battery, sorted the way `localeCompare` sorts it —
    /// including the one that bites this dev database: "Z.ai/…" after
    /// "openrouter/…" (case-folded), where byte order would flip them.
    #[test]
    fn catalog_sorts_like_localecompare_on_a_realistic_battery() {
        let id_battery = [
            "Z.ai/glm-5.3",
            "anthropic/claude-a",
            "openrouter/x",
            "Z.ai/abc",
            "zeta/m",
            "model-2",
            "model-10",
            "gpt-4o",
            "gpt-4.1",
            "o3",
            "o3-mini",
            "meta/llama-4",
            "meta-llama",
        ];
        // (qualified ids always gain an `<endpoint>/` prefix, so the battery is
        // sorted as raw strings — the pair test plus this covers the comparator,
        // and mixed_case_catalog_sort_is_not_byte_order covers catalog_of's use.)
        let mut sorted = id_battery.to_vec();
        sorted.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(
            sorted,
            [
                "anthropic/claude-a",
                "gpt-4.1",
                "gpt-4o",
                "meta-llama",
                "meta/llama-4",
                "model-10",
                "model-2",
                "o3",
                "o3-mini",
                "openrouter/x",
                "Z.ai/abc",
                "Z.ai/glm-5.3",
                "zeta/m",
            ]
        );
        // And byte order would disagree — the reason locale_cmp exists.
        let mut byte_sorted = id_battery.to_vec();
        byte_sorted.sort();
        assert_ne!(sorted, byte_sorted);
    }

    /// The divergence that bit the dev database, through catalog_of itself:
    /// "Z.ai/…" (uppercase Z) vs "openrouter/…" — ICU folds case, bytes don't.
    #[test]
    fn mixed_case_catalog_sort_is_not_byte_order() {
        let catalog = catalog_of(&[ep("Z.ai", &["glm-5.3"]), ep("openrouter", &["x"])]);
        let ids: Vec<&str> = catalog.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["openrouter/x", "Z.ai/glm-5.3"]);
    }

    /// A 400-id randomized battery (seeded LCG — regenerable from the
    /// generator command in git history) in the id grammar, sorted by real
    /// `localeCompare` and committed as a fixture, the same
    /// cross-language proof style as the secretbox vectors: the expectation
    /// was produced by the other implementation, not derived from this one.
    #[test]
    fn four_hundred_id_battery_sorts_like_node_localecompare() {
        #[derive(serde::Deserialize)]
        struct Battery {
            battery: Vec<String>,
            sorted: Vec<String>,
        }
        let fix: Battery =
            serde_json::from_str(include_str!("../../tests/fixtures/collation.json")).unwrap();
        assert_eq!(fix.battery.len(), 400);
        let mut sorted = fix.battery.clone();
        sorted.sort_by(|a, b| locale_cmp(a, b));
        // On a mismatch, name the first differing id — a 400-line assert is
        // unreadable otherwise.
        for (i, (got, want)) in sorted.iter().zip(&fix.sorted).enumerate() {
            assert_eq!(got, want, "position {i}: got {got:?}, node says {want:?}");
        }
    }
}
