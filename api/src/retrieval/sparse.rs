// Sparse (keyword) vectors for hybrid retrieval. Port of
// ui/src/server/retrieval/sparse.ts, line for line — this file is pure, which
// is the point of it: dense embeddings miss exact identifiers — ticket
// numbers, env vars, error strings, model names — so each chunk also gets a
// bag-of-terms vector: token → 32-bit hash index, value = saturated term
// frequency. Qdrant's sparse `modifier: idf` supplies the IDF half
// server-side at query time, so nothing here needs corpus statistics.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// The 86-word stop list, verbatim. Small enough that a hash set built once
/// beats any fancier structure; the words are English function words, and a
/// query composed entirely of them produces an empty vector — which matches
/// nothing, the honest answer for a query with no content words.
static STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "at", "for", "from", "by",
        "with", "as", "is", "are", "was", "were", "be", "been", "being", "it", "its", "this",
        "that", "these", "those", "i", "you", "he", "she", "we", "they", "them", "his", "her",
        "their", "our", "your", "my", "me", "us", "not", "no", "so", "if", "then", "than", "too",
        "very", "can", "will", "just", "do", "does", "did", "have", "has", "had", "what", "when",
        "where", "which", "who", "how", "why", "all", "any", "each", "there", "here", "about",
        "into", "over", "under", "again", "also", "up", "down", "out", "off",
    ]
    .into_iter()
    .collect()
});

/// FNV-1a over the token — a stable u32 index, hash-for-hash identical to the
/// TS (`h ^= charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0`), which is a
/// requirement, not a nicety: query-side and point-side vectors are encoded by
/// whichever runtime served the write, and a hash that drifted would make
/// every existing sparse point quietly unreachable. Collisions are rare
/// enough to be noise in a scoring context.
fn fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for unit in s.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The characters a token may keep. Everything else is a separator — which
/// makes every surviving token pure ASCII, so byte length and UTF-16 length
/// agree and the tokenizer never has to think about code units.
fn is_keep(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '@' | '.' | '-' | '/' | ':')
}

fn is_sep(c: char) -> bool {
    matches!(c, '_' | '.' | '-' | '/' | ':')
}

/// Tokens keep interior `.-_@/:` so identifiers survive whole:
/// `TALARIA_EMBED_MODEL`, `bge-small-en-v1.5`, `sk-proj-…`, `PL-142`.
fn tokens(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut raw = String::new();

    fn emit(raw: &str, out: &mut Vec<String>) {
        // Trim separator runs off both ENDS (never `_`): the class the TS
        // regex trims is `[.\-@/:]+` — a token that was all separators
        // disappears here, which is the `filter(Boolean)` arm.
        let t = raw.trim_matches(|c| matches!(c, '.' | '-' | '/' | ':' | '@'));
        // ASCII by construction, so byte length is the length the TS sees.
        if t.len() < 2 || STOPWORDS.contains(t) {
            return;
        }
        out.push(t.to_string());
        // An identifier also indexes under its parts, so "embed model" can
        // meet TALARIA_EMBED_MODEL halfway. The whole token stays the
        // strongest signal (it also carries its own tf count below).
        if t.contains(is_sep) {
            for part in t.split(is_sep) {
                if part.len() >= 2 && !STOPWORDS.contains(part) {
                    out.push(part.to_string());
                }
            }
        }
    }

    for c in lower.chars() {
        if is_keep(c) {
            raw.push(c);
        } else if !raw.is_empty() {
            emit(&raw, &mut out);
            raw.clear();
        }
    }
    if !raw.is_empty() {
        emit(&raw, &mut out);
    }
    out
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SparseVector {
    pub indices: Vec<u32>,
    pub values: Vec<f64>,
}

/// Encode text as a sparse vector: hashed terms with saturated tf (1 + ln n —
/// a term's tenth repeat shouldn't count like its first). Empty text → empty
/// vector (Qdrant accepts it; it simply matches nothing).
///
/// INDICES COME OUT IN FIRST-SEEN ORDER, matching the TS Map's insertion
/// order. Qdrant treats a sparse vector as an unordered multiset of
/// (index, value), so nothing downstream can observe the choice — but keeping
/// it identical costs nothing and makes a serialized vector byte-comparable
/// with the TS side during coexistence.
pub fn sparse_encode(text: &str) -> SparseVector {
    let mut order: Vec<u32> = Vec::new();
    let mut tf: HashMap<u32, u32> = HashMap::new();
    for t in tokens(text) {
        let h = fnv1a(&t);
        let count = tf.entry(h).or_insert(0);
        if *count == 0 {
            order.push(h);
        }
        *count += 1;
    }
    let values = order
        .iter()
        .map(|&h| 1.0 + f64::from(tf[&h]).ln())
        .collect();
    SparseVector {
        indices: order,
        values,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_the_published_vector_for_hello() {
        // 0x4F9F2CAB is the standard FNV-1a 32 of "hello" — pinning it pins
        // the whole encoding against a spelling accident.
        assert_eq!(fnv1a("hello"), 0x4F9F_2CAB);
    }

    #[test]
    fn identifiers_survive_whole_and_also_land_under_their_parts() {
        let got = tokens("the TALARIA_EMBED_MODEL failed");
        assert_eq!(
            got,
            vec!["talaria_embed_model", "talaria", "embed", "model", "failed"]
        );
        // A versioned id keeps its dots whole…
        assert!(tokens("bge-small-en-v1.5").contains(&"bge-small-en-v1.5".to_string()));
        // …and a ticket number keeps its whole form AND lands under its
        // parts, so the query "142" meets it halfway.
        assert_eq!(tokens("PL-142"), vec!["pl-142", "pl", "142"]);
    }

    #[test]
    fn separator_runs_split_and_bare_separators_disappear() {
        // Apostrophes aren't in the keep class: "what's" splits what + s —
        // and "what" IS on the stop list, s is too short, "up" is a stopword
        // too. An all-function-word query encodes to nothing, which matches
        // nothing — the honest answer.
        assert_eq!(tokens("what's up"), Vec::<String>::new());
        // Leading/trailing separator runs are trimmed (never `_`).
        assert_eq!(tokens("-ticket-"), vec!["ticket"]);
        assert_eq!(tokens("///..."), Vec::<String>::new());
        // Parts shorter than two never index: the whole token survives, the
        // single-letter parts under it do not.
        assert_eq!(tokens("x_y"), vec!["x_y"]);
        assert_eq!(tokens("a.b:c/d-e_f"), vec!["a.b:c/d-e_f"]);
    }

    #[test]
    fn stopwords_and_single_letters_never_index() {
        assert_eq!(tokens("the is a of to"), Vec::<String>::new());
        // "am" is NOT on the TS stop list (be/being/been are; am isn't) —
        // pin the gap so a well-meaning "fix" can't drift the vocabulary:
        // adding a word here would strand every point indexed before it.
        assert_eq!(tokens("I am at it"), vec!["am"]);
    }

    #[test]
    fn encode_weights_by_saturated_term_frequency() {
        let got = sparse_encode("error error error signal");
        // "error" seen three times → 1 + ln 3; "signal" once → 1 + ln 1 = 1
        // (a term's first occurrence counts at full weight — ln only dampens
        // repetition).
        let err_val = got.values[got
            .indices
            .iter()
            .position(|&h| h == fnv1a("error"))
            .unwrap()];
        let sig_val = got.values[got
            .indices
            .iter()
            .position(|&h| h == fnv1a("signal"))
            .unwrap()];
        assert!((err_val - (1.0 + 3f64.ln())).abs() < 1e-12);
        assert!((sig_val - 1.0).abs() < 1e-12);
        // First-seen order: error landed before signal.
        assert!(got.indices.first() == Some(&fnv1a("error")));
    }

    #[test]
    fn empty_text_is_an_empty_vector_not_an_error() {
        let got = sparse_encode("");
        assert!(got.indices.is_empty() && got.values.is_empty());
        assert_eq!(got, sparse_encode("   the of and  "));
    }

    #[test]
    fn encode_is_deterministic_and_order_stable() {
        let a = sparse_encode("TALARIA_EMBED_MODEL node 24 end of life");
        let b = sparse_encode("talaria_embed_model NODE 24 end of life");
        // Case-insensitive by construction; identical input → identical bytes.
        assert_eq!(a, b);
        assert_eq!(sparse_encode("node 24"), sparse_encode("node 24"));
    }
}
