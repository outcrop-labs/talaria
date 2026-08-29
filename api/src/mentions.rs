// Mentions — port of ui/src/server/mentions.ts, whole (it is 53 lines and
// the tasks/comment routes need it). Shared @mention handling across
// surfaces (channels, ticket comments, and — as they land — plans and
// research). A mention notifies any member the actor can reach; agent
// mentions are surfaced separately by each surface (channels trigger replies;
// comments pull the agent's attention).

use crate::notify::{NotificationInput, NotifyDeps, add_notification};

/// Tokens a person answers to: email localpart, dashed full name, first name.
fn user_mention_tokens(name: Option<&str>, email: Option<&str>) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    if let Some(email) = email
        && let Some(local) = email.split('@').next()
    {
        let local = local.to_lowercase();
        if !local.is_empty() && !tokens.contains(&local) {
            tokens.push(local);
        }
    }
    if let Some(n) = name.map(str::trim).filter(|n| !n.is_empty()) {
        let n = n.to_lowercase();
        let dashed = n.split_whitespace().collect::<Vec<_>>().join("-");
        if !dashed.is_empty() && !tokens.contains(&dashed) {
            tokens.push(dashed);
        }
        if let Some(first) = n.split_whitespace().next()
            && !tokens.contains(&first.to_string())
        {
            tokens.push(first.to_string());
        }
    }
    tokens
}

/// The @tokens present in a body (lowercased, without the leading @). TS's
/// regex is `/@([a-z0-9][a-z0-9-]*)/gi`: the first char after @ must be
/// alphanumeric, then alphanumerics and dashes.
fn mention_tokens(content: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' && i + 1 < bytes.len() {
            let rest = &content[i + 1..];
            let mut chars = rest.char_indices();
            let valid_first = chars.next().is_some_and(|(_, c)| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || c.is_ascii_uppercase()
            });
            if valid_first {
                let mut end = rest.len();
                for (idx, c) in rest.char_indices() {
                    let ok = c.is_ascii_lowercase()
                        || c.is_ascii_uppercase()
                        || c.is_ascii_digit()
                        || c == '-';
                    if !ok {
                        end = idx;
                        break;
                    }
                }
                let token = rest[..end].to_lowercase();
                if !token.is_empty() && !out.contains(&token) {
                    out.push(token);
                }
                // Resume scanning after the token — a '-' run inside one
                // token cannot start another.
                i += 1 + end;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A mentionable member (mentions.ts Mentionee) — the shape `list_members`
/// already returns.
pub struct Mentionee {
    pub user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Notify every member the body @mentions (never the sender). Generic over
/// the member set, so any surface with a membership list can reuse it.
/// Per-person notification failures are swallowed (logged) — one person's
/// failed row never costs the rest theirs, the same posture
/// notify_task_users takes.
pub async fn notify_mentions(
    notify: &NotifyDeps,
    members: &[Mentionee],
    sender_user_id: &str,
    sender_label: &str,
    content: &str,
    where_: &str,
    href: &str,
) {
    let mentions = mention_tokens(content);
    if mentions.is_empty() {
        return;
    }
    for m in members {
        if m.user_id == sender_user_id {
            continue;
        }
        let tokens = user_mention_tokens(m.name.as_deref(), m.email.as_deref());
        if !tokens.iter().any(|t| mentions.contains(t)) {
            continue;
        }
        let body = if content.chars().count() > 200 {
            let clipped: String = content.chars().take(200).collect();
            format!("{clipped}…")
        } else {
            content.to_string()
        };
        let input = NotificationInput {
            kind: "mention",
            title: &format!("{sender_label} mentioned you in {where_}"),
            body: Some(&body),
            href: Some(href),
        };
        if let Err(e) = add_notification(notify, &m.user_id, &input).await {
            tracing::error!("[mentions] notification for {} failed: {e}", m.user_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_cover_localpart_dashed_name_and_first_name() {
        assert_eq!(
            user_mention_tokens(Some("Ada Lovelace"), Some("Ada.L@example.com")),
            ["ada.l", "ada-lovelace", "ada"]
        );
        // No name, no email → nothing to answer to.
        assert!(user_mention_tokens(None, None).is_empty());
        // A whitespace-only name is no name.
        assert!(user_mention_tokens(Some("   "), None).is_empty());
        assert_eq!(user_mention_tokens(Some("Grace"), None), ["grace"]);
    }

    #[test]
    fn mention_tokens_match_the_ts_regex_shape() {
        // First char must be alphanumeric; then alphanumerics and dashes.
        assert_eq!(mention_tokens("hey @ada-l look"), ["ada-l"]);
        assert_eq!(mention_tokens("@Ada"), ["ada"]);
        // '@-foo' does not match (dash first) — the '@' is skipped and 'foo'
        // carries no @, so nothing is collected.
        assert!(mention_tokens("@-foo").is_empty());
        // An email-shaped run stops at the dot; the run after it is a fresh
        // candidate only with its own @.
        assert_eq!(mention_tokens("cc @ada.l and @bob"), ["ada", "bob"]);
        // Duplicates fold.
        assert_eq!(mention_tokens("@ada @ADA @ada"), ["ada"]);
        // A token boundary: '@ada-x' is one token, never 'ada' plus 'x'.
        assert_eq!(mention_tokens("@ada-x"), ["ada-x"]);
    }

    #[test]
    fn token_membership_matches_dashed_and_localpart_forms() {
        let tokens = user_mention_tokens(Some("Jon Iler"), Some("jon@x.com"));
        for t in ["jon", "jon-iler"] {
            assert!(tokens.contains(&t.to_string()), "missing {t}");
        }
    }
}
