// THE CITATION REGISTRY — one implementation, two hosts. Port of
// ui/src/server/source-registry.ts.
//
// The in-process research pipeline (research.ts, batch 5's tail) and the
// durable research run both renumber search hits onto one global source
// list, and each carried a private copy of this type until the copies
// diverged where it mattered: the run's `add` allocated `size + 1` where
// the pipeline's allocates HIGHEST + 1, so a run seeded from a parent whose
// source list carries a gap — size 3, highest [5] — handed [4] to a
// brand-new URL and silently re-aimed a citation a human already read.
// The TS side made this a shared leaf; the Rust side keeps it one so the
// research run def (the next slice of this batch) has the same floor.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

/// A citation marker, and it is not two digits. `\d{1,2}` was correct for
/// exactly as long as research meant Perplexity: sonar answers with a
/// handful of pre-ranked sources, so a registry never approached [99].
/// Research is model-agnostic now and the tool path is the common one — an
/// expedition is up to twelve queries against a web-search tool, each
/// returning a page of results, with every distinct URL numbered. Three
/// figures is ordinary there.
///
/// WHERE IT ACTUALLY BROKE, which is narrower than it first looks: both
/// failures were on the REPORT, whose markers are global — an invented
/// [150] survived the strip pass (never matched, so neither counted as
/// dropped nor removed), and the cited count undercounted, so
/// `report_problem` read an all-three-digit report as citing nothing.
/// `renumber` was never affected and never could be: the markers it
/// rewrites are LOCAL to one search hit, and it is the OUTPUT that carries
/// the global number.
///
/// Bounded at three digits rather than left open: `[2024]` in prose is a
/// year, and matching it would strip dates out of reports.
pub static MARKER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(\d{1,3})\]").expect("the marker grammar compiles"));

/// One row of the registry, as the report's Sources section prints it and
/// `research_sources` stores it (research.ts ResearchSource — the run's
/// RegistrySource was this same shape under a second name, and the
/// duplicate is gone on both sides of the port).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResearchSource {
    pub idx: u64,
    pub url: String,
    pub title: Option<String>,
    pub snippet: Option<String>,
}

/// A source as the search stages hand it over, before it has a number.
/// Declared here rather than imported from either host so this stays the
/// shared leaf (same reason as the TS module).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSeed {
    pub url: String,
    pub title: Option<String>,
    pub snippet: Option<String>,
}

#[derive(Default)]
struct Seeded {
    idx: u64,
    title: Option<String>,
    snippet: Option<String>,
}

/// The registry itself. Insertion order is `list()` order, so a HashMap
/// would scramble the Sources section — this is an insertion-ordered map
/// by hand (TS leaned on JS Map's insertion order for the same guarantee).
#[derive(Default)]
pub struct SourceRegistry {
    by_url: HashMap<String, Seeded>,
    order: Vec<String>,
}

impl SourceRegistry {
    /// SEED FROM A REPORT ALREADY WRITTEN, so a follow-up continues its
    /// numbering instead of starting again at [1]. This is what keeps the
    /// old text true: every [n] in the parent's prose points at a row in
    /// its source list, and renumbering — or reusing [3] for a new URL —
    /// would silently re-aim citations a human already read and believed.
    /// The parent's indices are taken verbatim and new sources continue
    /// from the highest, whatever gaps that leaves.
    ///
    /// Ascending `idx`, because insertion order is `list()` order: a
    /// registry rebuilt from an unsorted checkpoint renders its sources out
    /// of order even though every number still points where it always did.
    pub fn from(sources: &[ResearchSource]) -> SourceRegistry {
        let mut sorted = sources.to_vec();
        sorted.sort_by_key(|s| s.idx);
        let mut reg = SourceRegistry::default();
        for s in sorted {
            reg.insert(
                s.url,
                Seeded {
                    idx: s.idx,
                    title: s.title,
                    snippet: s.snippet,
                },
            );
        }
        reg
    }

    fn insert(&mut self, url: String, s: Seeded) {
        if !self.by_url.contains_key(&url) {
            self.order.push(url.clone());
        }
        self.by_url.insert(url, s);
    }

    /// Register a source, returning its global number. An existing URL
    /// keeps its number and only gains the title it was missing.
    pub fn add(&mut self, s: SourceSeed) -> u64 {
        if let Some(existing) = self.by_url.get_mut(&s.url) {
            if existing.title.is_none() && s.title.is_some() {
                existing.title = s.title;
            }
            return existing.idx;
        }
        // HIGHEST + 1, NOT SIZE + 1. A seeded registry can carry gaps — a
        // parent whose source [4] was deleted leaves size 3 and a highest
        // of 5 — and `size + 1` would hand [4] to a brand new URL, quietly
        // re-aiming every citation the parent's text makes to [4].
        let idx = self.by_url.values().map(|v| v.idx).max().unwrap_or(0) + 1;
        self.insert(
            s.url,
            Seeded {
                idx,
                title: s.title,
                snippet: s.snippet,
            },
        );
        idx
    }

    /// Rewrite one search hit's LOCAL [n] markers onto global numbering.
    /// A local number with no source behind it keeps its marker verbatim —
    /// an unnumbered citation is the synthesis stage's problem to catch,
    /// not this pass's to erase.
    pub fn renumber(&mut self, content: &str, sources: &[SourceSeed]) -> String {
        let mut map: HashMap<u64, u64> = HashMap::new();
        for (i, s) in sources.iter().enumerate() {
            map.insert((i + 1) as u64, self.add(s.clone()));
        }
        MARKER_RE
            .replace_all(content, |caps: &regex::Captures| {
                let n: u64 = caps[1].parse().unwrap_or(0);
                match map.get(&n) {
                    Some(g) => format!("[{g}]"),
                    None => caps[0].to_string(),
                }
            })
            .into_owned()
    }

    /// The registry in Source-section order (insertion order, which the
    /// ascending seed guarantees for a rebuilt registry and the `add`
    /// order guarantees for a fresh one).
    pub fn list(&self) -> Vec<ResearchSource> {
        self.order
            .iter()
            .filter_map(|url| {
                self.by_url.get(url).map(|s| ResearchSource {
                    idx: s.idx,
                    url: url.clone(),
                    title: s.title.clone(),
                    snippet: s.snippet.clone(),
                })
            })
            .collect()
    }

    pub fn size(&self) -> usize {
        self.by_url.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(url: &str, title: Option<&str>) -> SourceSeed {
        SourceSeed {
            url: url.into(),
            title: title.map(String::from),
            snippet: None,
        }
    }

    fn row(idx: u64, url: &str) -> ResearchSource {
        ResearchSource {
            idx,
            url: url.into(),
            title: None,
            snippet: None,
        }
    }

    #[test]
    fn a_seeded_registry_continues_from_the_highest_not_the_size() {
        // Size 3, highest [5]: `size + 1` would hand the next URL [4] and
        // re-aim the parent's [4] citations. Highest + 1 hands it [6].
        let mut reg = SourceRegistry::from(&[
            row(5, "https://e.test"),
            row(2, "https://b.test"),
            row(1, "https://a.test"),
        ]);
        assert_eq!(reg.size(), 3);
        assert_eq!(reg.add(seed("https://new.test", None)), 6);
    }

    #[test]
    fn seeding_sorts_so_list_order_follows_the_numbers() {
        // A checkpoint rebuilt from an unsorted list renders its Sources
        // section in insertion order — this is the guarantee `from` has to
        // restore before the first `list()`.
        let reg = SourceRegistry::from(&[
            row(3, "https://c.test"),
            row(1, "https://a.test"),
            row(2, "https://b.test"),
        ]);
        let idxs: Vec<u64> = reg.list().iter().map(|s| s.idx).collect();
        assert_eq!(idxs, vec![1, 2, 3]);
    }

    #[test]
    fn an_existing_url_keeps_its_number_and_only_gains_a_title() {
        let mut reg = SourceRegistry::default();
        assert_eq!(reg.add(seed("https://a.test", None)), 1);
        assert_eq!(reg.add(seed("https://a.test", Some("A"))), 1);
        assert_eq!(reg.list()[0].title.as_deref(), Some("A"));
        assert_eq!(reg.size(), 1, "the same URL twice is one source");
    }

    #[test]
    fn renumber_maps_local_markers_onto_global_numbers() {
        let mut reg = SourceRegistry::from(&[row(4, "https://parent.test")]);
        let hit = "Finding one [1] and finding two [2], citing the parent [4] too.";
        let out = reg.renumber(
            hit,
            &[
                seed("https://one.test", None),
                seed("https://parent.test", None),
            ],
        );
        // [1]→[5] (highest+1), and local 2 re-registers the parent's own
        // URL so it KEEPS [4] — the numbering the parent's text already
        // believes is the whole point of seeding.
        assert_eq!(
            out,
            "Finding one [5] and finding two [4], citing the parent [4] too."
        );
    }

    #[test]
    fn a_local_marker_with_no_source_keeps_its_marker() {
        let mut reg = SourceRegistry::default();
        let out = reg.renumber("claims [3] with nothing behind it", &[]);
        assert_eq!(out, "claims [3] with nothing behind it");
    }

    #[test]
    fn three_digit_markers_renumber_and_years_are_left_alone() {
        let mut reg = SourceRegistry::default();
        let mut sources = Vec::new();
        for i in 0..104 {
            sources.push(seed(&format!("https://s{i}.test"), None));
        }
        let out = reg.renumber("stable [104] since [2024] per [7]", &sources);
        assert_eq!(out, "stable [104] since [2024] per [7]");
        // The grammar is the point: [104] matches (three digits), [2024]
        // does not (four — a year in brackets, not a citation).
        assert!(MARKER_RE.is_match("[104]"));
        assert!(!MARKER_RE.is_match("[2024]"));
        assert!(!MARKER_RE.is_match("[1045]"));
    }
}
