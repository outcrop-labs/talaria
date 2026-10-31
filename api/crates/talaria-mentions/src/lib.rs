// Mentions — shared @mention handling across surfaces (channels, ticket
// comments, plans). A mention notifies any member the actor can reach; agent
// mentions are surfaced separately by each surface (channels trigger replies;
// comments pull the agent's attention).

use talaria_notify::{NotificationInput, NotifyDeps, add_notification, add_notification_capped};

/// Mention notifications one person receives from AGENTS per day. An agent
/// post that @mentions a member notifies exactly like a human's would, but a
/// human cannot post a thousand times a minute and an agent can — so the
/// agent-originated kind carries a per-recipient daily ceiling and the human
/// one does not.
pub const AGENT_MENTION_DAILY_CAP: i64 = 50;

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

/// The @tokens present in a body (lowercased, without the leading @): the
/// first char after @ must be alphanumeric, then alphanumerics and dashes.
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

/// A mentionable member — the shape `list_members`
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
///
/// `sender_user_id` EMPTY means the sender is not a person — every agent
/// call site (an agent's channel post, a streamed agent reply) passes `""`
/// because an agent has no user id — and those mentions are the ones the
/// per-recipient daily cap applies to.
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
    let to: Vec<&Mentionee> = members
        .iter()
        .filter(|m| m.user_id != sender_user_id && answers_to(m, &mentions))
        .collect();
    notify_these(
        notify,
        &to,
        sender_user_id,
        sender_label,
        content,
        where_,
        href,
    )
    .await;
}

/// Does this member answer to any of these @tokens?
fn answers_to(m: &Mentionee, mentions: &[String]) -> bool {
    user_mention_tokens(m.name.as_deref(), m.email.as_deref())
        .iter()
        .any(|t| mentions.contains(t))
}

/// The members an EDIT newly addresses: mentioned by the new text and not by
/// the old one, never the sender. By person, not by token — a member who was
/// `@ada` before and is `@ada-lovelace` now was already told. Pure, so the
/// rule is pinned by a test.
pub fn newly_mentioned<'a>(
    members: &'a [Mentionee],
    sender_user_id: &str,
    old: &str,
    new: &str,
) -> Vec<&'a Mentionee> {
    let now = mention_tokens(new);
    if now.is_empty() {
        return Vec::new();
    }
    let before = mention_tokens(old);
    members
        .iter()
        .filter(|m| m.user_id != sender_user_id)
        .filter(|m| answers_to(m, &now) && !answers_to(m, &before))
        .collect()
}

/// `notify_mentions` for an EDIT of text that already notified once (a
/// ticket description): only the people the edit ADDS are told. Re-running
/// the whole text on every save filed a fresh "mentioned you" for every
/// mention already there — 55 rapid edits were 55 rows, 55 pushes and, by
/// default, 55 emails.
#[allow(clippy::too_many_arguments)]
pub async fn notify_added_mentions(
    notify: &NotifyDeps,
    members: &[Mentionee],
    sender_user_id: &str,
    sender_label: &str,
    old_content: &str,
    new_content: &str,
    where_: &str,
    href: &str,
) {
    let to = newly_mentioned(members, sender_user_id, old_content, new_content);
    notify_these(
        notify,
        &to,
        sender_user_id,
        sender_label,
        new_content,
        where_,
        href,
    )
    .await;
}

async fn notify_these(
    notify: &NotifyDeps,
    to: &[&Mentionee],
    sender_user_id: &str,
    sender_label: &str,
    content: &str,
    where_: &str,
    href: &str,
) {
    for m in to {
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
        let written = if sender_user_id.is_empty() {
            add_notification_capped(notify, &m.user_id, &input, AGENT_MENTION_DAILY_CAP).await
        } else {
            add_notification(notify, &m.user_id, &input)
                .await
                .map(|()| true)
        };
        if let Err(e) = written {
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
    fn mention_tokens_match_the_regex_shape() {
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

    fn member(id: &str, name: &str, email: &str) -> Mentionee {
        Mentionee {
            user_id: id.into(),
            name: Some(name.into()),
            email: Some(email.into()),
        }
    }

    #[test]
    fn an_edit_notifies_only_the_people_it_adds() {
        let members = [
            member("a", "Ada Lovelace", "ada@x.test"),
            member("c", "Cy Young", "cy@x.test"),
            member("s", "Sam Sender", "sam@x.test"),
        ];
        let ids = |v: Vec<&Mentionee>| v.iter().map(|m| m.user_id.clone()).collect::<Vec<_>>();
        // A punctuation edit with the same mention: nobody.
        assert!(newly_mentioned(&members, "s", "@ada look", "@ada look!").is_empty());
        // Adding @cy: Cy only.
        assert_eq!(
            ids(newly_mentioned(
                &members,
                "s",
                "@ada look",
                "@ada and @cy look"
            )),
            ["c"]
        );
        // Another token for someone already mentioned is not news.
        assert!(newly_mentioned(&members, "s", "@ada", "@ada-lovelace").is_empty());
        // Removing a mention notifies nobody.
        assert!(newly_mentioned(&members, "s", "@ada @cy", "@ada").is_empty());
        // The sender never hears about themselves.
        assert!(newly_mentioned(&members, "s", "", "@sam").is_empty());
        // From an empty description, every mention is new.
        assert_eq!(
            ids(newly_mentioned(&members, "s", "", "@ada @cy")),
            ["a", "c"]
        );
    }

    #[test]
    fn token_membership_matches_dashed_and_localpart_forms() {
        let tokens = user_mention_tokens(Some("Jon Iler"), Some("jon@x.com"));
        for t in ["jon", "jon-iler"] {
            assert!(tokens.contains(&t.to_string()), "missing {t}");
        }
    }
}
