// ARMING A PROVIDER'S OWN WEB SEARCH, and reading the citations back.
// Port of ui/src/server/native-search.ts.
//
// WHY THIS FILE EXISTS. Talaria's research pipeline can search two ways: through
// a tool it drives itself (SearXNG or a registered MCP server), or by letting
// the model search as part of answering. The second one was never actually
// switched on. `build_upstream` forwards a plain chat body, so "native" meant
// posting an ordinary completion to a model that COULD have searched and
// harvesting whatever it volunteered — which in practice is Perplexity's sonar
// family, because sonar always searches, and nothing else.
//
// That is the whole reason research used to require Perplexity.
//
// WHAT IS ACTUALLY REACHABLE from a gateway that speaks OpenAI-shaped bodies,
// established from each provider's current documentation rather than from
// memory. It is less than one would hope, and saying so here is the point:
//
//   PERPLEXITY    already on. Sonar searches unconditionally; there is no
//                 parameter to send and never was. Citations come back OUT OF
//                 BAND at the response root.
//   OPENROUTER    yes, and this is the one worth having: `plugins: [{id:'web'}]`
//                 searches once per request for ANY model, and the catalog says
//                 which models have provider-native search of their own. This is
//                 how most self-hosted installs reach a frontier model.
//   OPENAI        only by picking a `-search-api` MODEL. `web_search_options` is
//                 a TUNER, not a switch, and it 400s on a model that does not
//                 have search — so sending it hopefully would break every
//                 ordinary call. The real controls live on the Responses API,
//                 which is a different endpoint and a different body shape.
//   ANTHROPIC     no. Its web search is a server tool on the native `/v1/messages`
//                 body, and the OpenAI compatibility layer does not expose it.
//                 A gateway forwarding OpenAI bodies cannot arm it at all.
//
// SO THE TOOL PATH IS THE UNIVERSAL ONE, and native is the optimisation. That is
// the opposite of how this pipeline was built, and it is why `planSearch` now
// falls back to the tool path for any model rather than refusing.
//
// NOTHING HERE IS A MODEL LIST. Provider is read off the endpoint and capability
// off the catalog, so a model that gains search next month is picked up the day
// it is registered.

use serde_json::{Map, Value};

/// `provider` as it is spelled on the endpoint row. Compared case-insensitively
/// and by prefix, because an operator naming an endpoint "openrouter-prod" means
/// OpenRouter and should not have to know that we string-match.
///
/// TOLERATES A MISSING PROVIDER, which is not defensive padding: an endpoint row
/// can carry an empty provider, and the honest answer for "we do not know what
/// this is" is to send nothing rather than to panic inside a search stage.
fn is_provider(provider: Option<&str>, want: &str) -> bool {
    let p = provider.unwrap_or("").trim().to_lowercase();
    if p.is_empty() {
        return false;
    }
    let dash = format!("{want}-");
    let underscore = format!("{want}_");
    p == want || p.starts_with(&dash) || p.starts_with(&underscore) || p.contains(want)
}

/// ARM IT, if this provider can be armed over an OpenAI-shaped body. What to
/// merge into the request body — an empty map sends nothing, which includes both
/// "this provider cannot be armed" and "it is already on".
///
/// TAKES THE PROVIDER AND NOTHING ELSE, which is a smaller signature than the
/// first draft had. It wanted the model's own catalog capability too — until it
/// turned out that neither branch which could use it wants it: OpenRouter's web
/// plugin works on ANY model whether or not that model has search of its own,
/// and OpenAI has no safe parameter at any capability level. An argument no
/// branch reads is an argument the next reader has to disprove.
///
/// DELIBERATELY CONSERVATIVE. Every branch that is not certain sends nothing:
/// an unrecognised provider, an OpenAI model without search, anything Anthropic.
/// Sending a parameter a provider rejects turns a working research run into a
/// 400, and the failure mode of sending nothing is merely the tool path — which
/// is where most runs go anyway.
pub fn native_search_body(provider: Option<&str>) -> Map<String, Value> {
    let mut out = Map::new();
    // OpenRouter: one search per request, any model, normalised citations back.
    // The `web` plugin is documented as deprecated in favour of the server tool,
    // and is kept because it is the one that does not depend on the MODEL
    // choosing to call a tool — a search stage that may or may not have searched
    // is not a search stage. Revisit when the server tool can be forced.
    if is_provider(provider, "openrouter") {
        out.insert("plugins".into(), serde_json::json!([{ "id": "web" }]));
    }
    // Perplexity searches unconditionally; OpenAI's switch is the MODEL, not a
    // parameter (`web_search_options` 400s on a model without search); Anthropic
    // and everything unrecognised have no OpenAI-shaped activation at all. Every
    // one of those branches sends nothing, and the TS file spells each no out
    // loud — here the empty map says them together.
    out
}

/// Whether this provider's native search can be armed at all from here. Used by
/// the admin-facing explanation, so an operator is told "this model can search
/// but we cannot switch it on over this endpoint" rather than being left to
/// wonder why every run takes the tool path.
pub fn can_arm_native(provider: Option<&str>) -> bool {
    is_provider(provider, "openrouter") || is_provider(provider, "perplexity")
}

/// A citation as the search stage records it, whatever envelope it arrived in.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestedSource {
    pub url: String,
    pub title: Option<String>,
    pub snippet: Option<String>,
}

/// EVERY SHAPE A CITATION COMES BACK IN, normalised.
///
/// THE BUG THIS FIXES. The search stage only ever read Perplexity's two
/// out-of-band fields, so a model that searched and cited through the OPENAI
/// ANNOTATION SHAPE — which is what OpenAI returns and what OpenRouter
/// normalises every one of its engines to — recorded ZERO sources. The run then
/// had findings with nothing to cite, and the fixtures scored the model for
/// writing an uncited brief. The model had done its job; we dropped the evidence.
///
/// Three shapes, all real, in the order they are most likely to be authoritative:
///   `search_results`  Perplexity, richest (title + snippet + url)
///   `annotations`     OpenAI / OpenRouter, per-message, `url_citation`
///   `citations`       Perplexity's older flat list of URLs
///
/// MERGED, NOT FIRST-MATCH-WINS, because a response may legitimately carry more
/// than one — and deduped on URL, because the same source appearing in two
/// shapes is one source, not two. First spelling of a URL keeps its title.
///
/// A NON-OBJECT BODY IS A REAL CASE, not defensive padding: this parses a live
/// provider response, and a gateway erroring can return `null`, a string, or an
/// HTML page. Failing here would turn "the provider had a bad minute" into a
/// failed research run instead of a retry.
pub fn harvest_sources(body: &Value) -> Vec<HarvestedSource> {
    let Some(j) = body.as_object() else {
        return Vec::new();
    };
    let mut out: Vec<HarvestedSource> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    fn non_empty(s: Option<&str>) -> Option<String> {
        // TS `typeof x === 'string' && x !== ''` — a blank string is absent and
        // a whitespace string is present; neither is trimmed here.
        s.filter(|s| !s.is_empty()).map(str::to_string)
    }

    // (url, title, snippet) with the URL's own gate: a non-string or
    // blank-after-trim URL is not a citation.
    let add = |url: Option<&str>,
               title: Option<&str>,
               snippet: Option<&str>,
               out: &mut Vec<HarvestedSource>,
               seen: &mut std::collections::HashSet<String>| {
        let url = url.unwrap_or("").trim();
        if url.is_empty() || seen.contains(url) {
            return;
        }
        seen.insert(url.to_string());
        out.push(HarvestedSource {
            url: url.to_string(),
            title: non_empty(title),
            snippet: non_empty(snippet),
        });
    };

    for s in j
        .get("search_results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let o = s.as_object();
        add(
            o.and_then(|o| o.get("url")).and_then(Value::as_str),
            o.and_then(|o| o.get("title")).and_then(Value::as_str),
            o.and_then(|o| o.get("snippet")).and_then(Value::as_str),
            &mut out,
            &mut seen,
        );
    }
    for c in j
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for a in c
            .as_object()
            .and_then(|c| c.get("message"))
            .and_then(Value::as_object)
            .and_then(|m| m.get("annotations"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let a = a.as_object();
            // `type` is the discriminator and there are others (`file_citation`),
            // so a reader that took every annotation would file a local file
            // path as a web source. A FALSY type — absent, null, or '' — is
            // url_citation by omission, the TS truthiness check rather than a
            // presence check.
            let kind = a.and_then(|a| a.get("type")).and_then(Value::as_str);
            if kind.is_some_and(|t| !t.is_empty() && t != "url_citation") {
                continue;
            }
            let cit = a
                .and_then(|a| a.get("url_citation"))
                .and_then(Value::as_object);
            add(
                cit.and_then(|c| c.get("url")).and_then(Value::as_str),
                cit.and_then(|c| c.get("title")).and_then(Value::as_str),
                cit.and_then(|c| c.get("content")).and_then(Value::as_str),
                &mut out,
                &mut seen,
            );
        }
    }
    // Old Perplexity: a flat list of URL strings. Tolerates the object form too,
    // because more than one gateway has been seen to normalise it that way.
    for c in j
        .get("citations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match c {
            Value::String(s) => add(Some(s), None, None, &mut out, &mut seen),
            other => {
                let o = other.as_object();
                add(
                    o.and_then(|o| o.get("url")).and_then(Value::as_str),
                    o.and_then(|o| o.get("title")).and_then(Value::as_str),
                    None,
                    &mut out,
                    &mut seen,
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_matching_takes_the_spellings_an_operator_actually_types() {
        for p in [
            "openrouter",
            "OpenRouter",
            "openrouter-prod",
            "openrouter_prod",
            "anOpenRouterMirror",
        ] {
            assert!(is_provider(Some(p), "openrouter"), "{p}");
        }
        // Missing, blank, and unrelated: the honest answer is no, not a panic —
        // an endpoint row can carry an empty provider.
        for p in [None, Some(""), Some("   "), Some("openai")] {
            assert!(!is_provider(p, "openrouter"), "{p:?}");
        }
    }

    #[test]
    fn only_openrouter_is_armed_and_the_rest_say_nothing() {
        let armed = native_search_body(Some("openrouter-prod"));
        assert_eq!(armed.get("plugins"), Some(&json!([{ "id": "web" }])));
        assert_eq!(armed.len(), 1);
        // Perplexity is already on; OpenAI's switch is the model; Anthropic has
        // no activation; unknown sends nothing rather than a guess.
        for p in [
            "perplexity",
            "sonar",
            "openai",
            "gpt-5-search-api",
            "anthropic",
            "",
            "mystery",
        ] {
            assert!(native_search_body(Some(p)).is_empty(), "{p}");
        }
        assert!(native_search_body(None).is_empty());
    }

    #[test]
    fn can_arm_is_the_two_providers_whose_search_is_switchable() {
        assert!(can_arm_native(Some("openrouter")));
        assert!(can_arm_native(Some("Perplexity")));
        assert!(!can_arm_native(Some("openai")));
        assert!(!can_arm_native(None));
    }

    #[test]
    fn harvest_reads_all_three_shapes_and_merges_them() {
        // One source in each shape, one URL in two shapes (one entry), plus a
        // rich search_results row and an annotation carrying a content snippet.
        let body = json!({
            "search_results": [
                { "url": "https://a.test/post", "title": "The post", "snippet": "what it said" },
                { "url": "", "title": "no url is not a citation" },
                "not an object"
            ],
            "choices": [{ "message": { "annotations": [
                { "type": "url_citation", "url_citation": { "url": "https://b.test/", "title": "B", "content": "the passage" } },
                { "url_citation": { "url": "https://a.test/post", "title": "same url twice is one source" } },
                { "type": "file_citation", "file_citation": { "file_id": "file_1" } },
                { "type": "", "url_citation": { "url": "https://c.test" } }
            ] } }],
            "citations": [
                "https://d.test/flat",
                { "url": "https://e.test/object", "title": "E" },
                { "title": "an object with no url is dropped" }
            ]
        });
        let got = harvest_sources(&body);
        let urls: Vec<&str> = got.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://a.test/post",
                "https://b.test/",
                "https://c.test",
                "https://d.test/flat",
                "https://e.test/object",
            ]
        );
        // The first spelling of a URL keeps its title: the annotation's repeat
        // of a.test/post changed nothing.
        assert_eq!(got[0].title.as_deref(), Some("The post"));
        assert_eq!(got[0].snippet.as_deref(), Some("what it said"));
        assert_eq!(got[1].snippet.as_deref(), Some("the passage"));
        // Blank strings are absent, not empty titles.
        assert_eq!(got[2].title, None);
        assert_eq!(got[3].title, None);
        assert_eq!(got[4].title.as_deref(), Some("E"));
    }

    #[test]
    fn a_non_object_body_harvests_nothing_rather_than_failing() {
        // A gateway erroring can return null, a string, or an HTML page.
        assert!(harvest_sources(&Value::Null).is_empty());
        assert!(harvest_sources(&json!("gateway unavailable")).is_empty());
        assert!(harvest_sources(&json!("<html>502</html>")).is_empty());
        assert!(harvest_sources(&json!({})).is_empty());
    }
}
