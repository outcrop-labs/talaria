// Marketplace icon cache — port of ui/src/server/mcp-icons.ts. Icons resolve
// server-side (declared icon URL, else the DuckDuckGo favicon CDN — fast —
// else the site's own favicon) and are WARMED in bulk whenever a library page
// is served, so cards paint from cache instead of fanning out cold fetches
// per image.
//
// Both inputs are attacker-reachable: `domain` comes straight off a signed-in
// user's query, and `src` off the public MCP registry. The proxy then returns
// the BYTES it fetched, so an unguarded fetch here is a read primitive against
// the private network. `public_icon_domain` screens the domain candidates but
// says nothing about `src`, and neither screens what a redirect lands on —
// safe_fetch (redirects re-validated per hop, size capped) is what keeps this
// a favicon fetcher.
//
// State lives inside a constructed IconCache (edge + clock injectable, so
// tests fake both); `icons()` is the production singleton on safe_fetch.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use futures_util::StreamExt;
use futures_util::future::BoxFuture;

pub const ICON_CACHE_MS: i64 = 24 * 60 * 60 * 1000;
pub const ICON_MAX_BYTES: u64 = 512 * 1024;
/// TS's icon workers — same bound as the registry pool.
const WARM_CONCURRENCY: usize = 6;

/// The icon src (registry-declared) or the domain to favicon-proxy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconKey {
    pub src: Option<String>,
    pub domain: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct Icon {
    pub buf: Vec<u8>,
    pub content_type: String,
}

/// The outbound edge: (status, content-type, body) for a URL. safe_fetch in
/// production, faked in tests.
pub type FetchIcon =
    Arc<dyn Fn(&str) -> BoxFuture<'static, Result<(u16, String, Vec<u8>), String>> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Clone)]
enum Entry {
    Hit { at: i64, icon: Arc<Icon> },
    Miss { at: i64 },
}

pub struct IconCache {
    fetch: FetchIcon,
    now: NowFn,
    cache: Mutex<HashMap<String, Entry>>,
}

/// ui/src/lib/icon-domain.ts — only public, dotted hostnames can plausibly
/// serve a favicon. Internal endpoints — IP literals (the built-in toolkit's
/// local gateway), single-label hosts from pseudo-URLs like
/// `talaria-workbench://core`, localhost and friends — would only ever 404 the
/// icon proxy, so callers skip the request entirely.
pub fn public_icon_domain(domain: Option<&str>) -> Option<String> {
    let d = domain?.trim().to_lowercase();
    if d.is_empty() || !d.contains('.') {
        return None;
    }
    if d == "localhost"
        || d.ends_with(".localhost")
        || d.ends_with(".local")
        || d.ends_with(".internal")
    {
        return None;
    }
    // IPv4 literal: exactly four 1-3 digit groups.
    let parts: Vec<&str> = d.split('.').collect();
    if parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    // IPv6 literal or anything else port/bracket-shaped.
    if d.starts_with('[') || d.contains(':') {
        return None;
    }
    Some(d)
}

async fn fetch_icon(fetch: &FetchIcon, url: &str) -> Option<Icon> {
    let (status, ct, buf) = fetch(url).await.ok()?;
    if !(200..300).contains(&status) {
        return None;
    }
    if !ct.starts_with("image/") {
        return None;
    }
    if buf.is_empty() || buf.len() as u64 > ICON_MAX_BYTES {
        return None;
    }
    Some(Icon {
        buf,
        content_type: ct,
    })
}

fn candidates_for(key: &IconKey) -> Vec<String> {
    if let Some(src) = &key.src {
        return vec![src.clone()];
    }
    match public_icon_domain(key.domain.as_deref()) {
        Some(domain) => vec![
            format!("https://icons.duckduckgo.com/ip3/{domain}.ico"),
            format!("https://{domain}/favicon.ico"),
        ],
        None => Vec::new(),
    }
}

impl IconCache {
    pub fn new(fetch: FetchIcon, now: NowFn) -> IconCache {
        IconCache {
            fetch,
            now,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn fresh_entry(&self, id: &str) -> Option<Entry> {
        let hit = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()?;
        let at = match &hit {
            Entry::Hit { at, .. } => *at,
            Entry::Miss { at } => *at,
        };
        ((self.now)() - at < ICON_CACHE_MS).then_some(hit)
    }

    pub async fn resolve_icon(&self, key: &IconKey) -> Option<Arc<Icon>> {
        let id = key.src.clone().or_else(|| key.domain.clone())?;
        if let Some(entry) = self.fresh_entry(&id) {
            return match entry {
                Entry::Hit { icon, .. } => Some(icon),
                Entry::Miss { .. } => None,
            };
        }
        for candidate in candidates_for(key) {
            if let Some(icon) = fetch_icon(&self.fetch, &candidate).await {
                let icon = Arc::new(icon);
                self.cache.lock().unwrap_or_else(|p| p.into_inner()).insert(
                    id,
                    Entry::Hit {
                        at: (self.now)(),
                        icon: icon.clone(),
                    },
                );
                return Some(icon);
            }
        }
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, Entry::Miss { at: (self.now)() });
        None
    }

    /// Fire-and-forget bulk warm — bounded concurrency, silent failures.
    /// Arc-received so the detached task shares THIS cache, not a copy.
    pub fn warm(self: &Arc<Self>, keys: Vec<IconKey>) {
        let fresh: Vec<IconKey> = keys
            .into_iter()
            .filter(|k| {
                let Some(id) = k.src.clone().or_else(|| k.domain.clone()) else {
                    return false;
                };
                self.fresh_entry(&id).is_none()
            })
            .collect();
        if fresh.is_empty() {
            return;
        }
        let cache = Arc::clone(self);
        tokio::spawn(async move {
            futures_util::stream::iter(fresh)
                .for_each_concurrent(WARM_CONCURRENCY, |k| {
                    let cache = Arc::clone(&cache);
                    async move {
                        let _ = cache.resolve_icon(&k).await;
                    }
                })
                .await;
        });
    }
}

/// The production singleton: safe_fetch edge (5s timeout, 512KB cap — the
/// TS fetchIcon bounds), wall clock.
pub fn icons() -> Arc<IconCache> {
    static ICONS: LazyLock<Arc<IconCache>> = LazyLock::new(|| {
        let fetch: FetchIcon = Arc::new(|url: &str| {
            let url = url.to_string();
            Box::pin(async move {
                let r = crate::safe_fetch::safe_fetch(
                    &url,
                    crate::safe_fetch::SafeFetch {
                        timeout_ms: Some(5_000),
                        max_bytes: Some(ICON_MAX_BYTES),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                let ct = r
                    .headers
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                Ok((r.status, ct, r.body))
            })
        });
        let now: NowFn = Arc::new(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        });
        Arc::new(IconCache::new(fetch, now))
    });
    Arc::clone(&ICONS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn png(bytes: &[u8]) -> Result<(u16, String, Vec<u8>), String> {
        Ok((200, "image/png".into(), bytes.to_vec()))
    }

    #[test]
    fn only_public_dotted_hostnames_are_icon_candidates() {
        assert_eq!(
            public_icon_domain(Some("Stripe.com ")),
            Some("stripe.com".into())
        );
        assert_eq!(
            public_icon_domain(Some("mcp.vercel.com")),
            Some("mcp.vercel.com".into())
        );
        assert_eq!(public_icon_domain(None), None);
        assert_eq!(public_icon_domain(Some("  ")), None);
        assert_eq!(public_icon_domain(Some("localhost")), None);
        assert_eq!(public_icon_domain(Some("talaria-workbench")), None); // single label
        assert_eq!(public_icon_domain(Some("127.0.0.1")), None); // IPv4 literal
        assert_eq!(public_icon_domain(Some("999.1.1.1")), None); // digit groups, still literal-shaped
        assert_eq!(public_icon_domain(Some("[::1]")), None);
        assert_eq!(public_icon_domain(Some("a.b.internal")), None);
        assert_eq!(public_icon_domain(Some("a.b:8443")), None);
    }

    #[test]
    fn candidates_prefer_the_declared_src_then_the_favicon_chain() {
        let k = IconKey {
            src: Some("https://x/y.png".into()),
            domain: Some("stripe.com".into()),
        };
        assert_eq!(candidates_for(&k), ["https://x/y.png"]);
        let k = IconKey {
            src: None,
            domain: Some("stripe.com".into()),
        };
        assert_eq!(
            candidates_for(&k),
            [
                "https://icons.duckduckgo.com/ip3/stripe.com.ico",
                "https://stripe.com/favicon.ico"
            ]
        );
        let k = IconKey {
            src: None,
            domain: Some("box.internal".into()),
        };
        assert!(candidates_for(&k).is_empty());
    }

    #[tokio::test]
    async fn a_resolved_icon_caches_and_a_miss_caches_as_a_miss() {
        let fetches = Arc::new(AtomicUsize::new(0));
        let f = fetches.clone();
        let fetch: FetchIcon = Arc::new(move |url: &str| {
            let f = f.clone();
            let url = url.to_string();
            Box::pin(async move {
                f.fetch_add(1, Ordering::SeqCst);
                if url.contains("duckduckgo") {
                    Ok((404, "text/plain".into(), vec![]))
                } else {
                    png(&[1, 2, 3])
                }
            })
        });
        let clock = Arc::new(Mutex::new(1_000i64));
        let now: NowFn = {
            let clock = clock.clone();
            Arc::new(move || *clock.lock().unwrap_or_else(|p| p.into_inner()))
        };
        let cache = IconCache::new(fetch, now);

        let key = IconKey {
            src: None,
            domain: Some("stripe.com".into()),
        };
        let icon = cache.resolve_icon(&key).await.unwrap();
        assert_eq!(icon.content_type, "image/png");
        assert_eq!(icon.buf, vec![1, 2, 3]);
        // The DuckDuckGo miss then the site hit: two fetches, cached after.
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        let again = cache.resolve_icon(&key).await.unwrap();
        assert_eq!(again.buf, vec![1, 2, 3]);
        assert_eq!(fetches.load(Ordering::SeqCst), 2);

        // Age past 24h: re-resolved (the TTL works).
        *clock.lock().unwrap_or_else(|p| p.into_inner()) += ICON_CACHE_MS;
        cache.resolve_icon(&key).await.unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn a_non_image_or_oversized_answer_is_a_miss_not_an_icon() {
        let fetch: FetchIcon = Arc::new(|_url: &str| {
            Box::pin(async { Ok((200, "text/html".into(), b"<html>".to_vec())) })
        });
        let now: NowFn = Arc::new(|| 0);
        let cache = IconCache::new(fetch, now);
        let key = IconKey {
            src: Some("https://x/y".into()),
            domain: None,
        };
        assert!(cache.resolve_icon(&key).await.is_none());
        // …and the miss is cached: no second fetch happens on re-ask.
        assert!(cache.resolve_icon(&key).await.is_none());
    }
}
