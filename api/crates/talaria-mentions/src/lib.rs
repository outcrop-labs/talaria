// Mentions — shared @mention handling across surfaces (channels, ticket
// comments, plans). A mention notifies any member the actor can reach; agent
// mentions are surfaced separately by each surface (channels trigger replies;
// comments pull the agent's attention).

use std::collections::HashSet;

use talaria_notify::{
    ActorKind, NotificationInput, NotifyActor, NotifyDeps, NotifyEntity, NotifyMeta,
    add_notification_with,
};
// The token rules live beside the other audience questions in
// talaria-notify, so a producer that cannot depend on this crate (a ticket's
// comment participants) asks the same ones.
use talaria_notify::{mention_tokens, user_mention_tokens};

/// Mention notifications one person receives from AGENTS per day. An agent
/// post that @mentions a member notifies exactly like a human's would, but a
/// human cannot post a thousand times a minute and an agent can — so the
/// agent-originated kind carries a per-recipient daily ceiling and the human
/// one does not.
pub const AGENT_MENTION_DAILY_CAP: i64 = 50;

/// A mentionable member — the shape `list_members`
/// already returns.
pub struct Mentionee {
    pub user_id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// Who wrote the text that @mentions people.
#[derive(Clone, Copy, Debug)]
pub struct MentionSender<'a> {
    /// The person's `users.id`. EMPTY when the sender is not a person — an
    /// agent has no user id — and those mentions are the ones the
    /// per-recipient daily cap applies to.
    pub user_id: &'a str,
    /// What the title calls them.
    pub label: &'a str,
    /// The agent's fleet model id, when an agent wrote it (the row's actor).
    pub agent_model: Option<&'a str>,
}

impl<'a> MentionSender<'a> {
    /// A person, or — with an empty id — an agent known only by its label.
    pub fn of(user_id: &'a str, label: &'a str) -> Self {
        Self {
            user_id,
            label,
            agent_model: None,
        }
    }

    /// Who the row says caused it (I-11): the person, else the agent.
    fn actor(&self) -> NotifyActor {
        if self.user_id.is_empty() {
            NotifyActor {
                kind: ActorKind::Agent,
                id: self.agent_model.unwrap_or(self.label).to_string(),
                label: self.label.to_string(),
            }
        } else {
            NotifyActor {
                kind: ActorKind::User,
                id: self.user_id.to_string(),
                label: self.label.to_string(),
            }
        }
    }
}

/// What a surface adds to its mention rows: the thing they are about, and the
/// members whose mute is running (their rows land quietly — counted, never
/// toasted, bannered, pushed or mailed).
#[derive(Clone, Debug, Default)]
pub struct MentionOpts {
    pub entity: Option<NotifyEntity>,
    pub quiet: HashSet<String>,
}

/// The members a body @mentions, never the sender — exactly who
/// `notify_mentions` would tell. A surface that files a second kind of row
/// for the same message (a ticket comment's participants, a channel at level
/// `all`) asks this first, so nobody gets two rows for one message.
pub fn mentioned_ids(members: &[Mentionee], sender_user_id: &str, content: &str) -> Vec<String> {
    let mentions = mention_tokens(content);
    if mentions.is_empty() {
        return Vec::new();
    }
    members
        .iter()
        .filter(|m| m.user_id != sender_user_id && answers_to(m, &mentions))
        .map(|m| m.user_id.clone())
        .collect()
}

/// Notify every member the body @mentions (never the sender). Generic over
/// the member set, so any surface with a membership list can reuse it.
/// Per-person notification failures are swallowed (logged) — one person's
/// failed row never costs the rest theirs, the same posture
/// notify_task_users takes. Answers who was told.
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
) -> Vec<String> {
    notify_mentions_with(
        notify,
        members,
        &MentionSender::of(sender_user_id, sender_label),
        content,
        where_,
        href,
        &MentionOpts::default(),
    )
    .await
}

/// `notify_mentions` with the row's entity, the sender's agent id and the
/// members heard quietly. Answers who was told.
pub async fn notify_mentions_with(
    notify: &NotifyDeps,
    members: &[Mentionee],
    sender: &MentionSender<'_>,
    content: &str,
    where_: &str,
    href: &str,
    opts: &MentionOpts,
) -> Vec<String> {
    let mentions = mention_tokens(content);
    if mentions.is_empty() {
        return Vec::new();
    }
    let to: Vec<&Mentionee> = members
        .iter()
        .filter(|m| m.user_id != sender.user_id && answers_to(m, &mentions))
        .collect();
    notify_these(notify, &to, sender, content, where_, href, opts).await
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
    sender: &MentionSender<'_>,
    old_content: &str,
    new_content: &str,
    where_: &str,
    href: &str,
    opts: &MentionOpts,
) -> Vec<String> {
    let to = newly_mentioned(members, sender.user_id, old_content, new_content);
    notify_these(notify, &to, sender, new_content, where_, href, opts).await
}

/// How one mention row files: no group key (each mention is its own row —
/// somebody pointed at you on purpose), about the surface's entity, from the
/// sender, quiet when the person's mute is running. Pure.
fn mention_meta(sender: &MentionSender<'_>, opts: &MentionOpts, user_id: &str) -> NotifyMeta {
    NotifyMeta {
        quiet: opts.quiet.contains(user_id),
        entity: opts.entity.clone(),
        actor: Some(sender.actor()),
        ..Default::default()
    }
}

/// The agent-originated cap (`AGENT_MENTION_DAILY_CAP`): whether this person
/// holds fewer than the cap of `mention` rows from the last day. The writer's
/// own `add_notification_capped` makes the same read; it is spelled here
/// because a capped write with an actor and an entity goes through
/// `add_notification_with`. Read-then-insert on purpose (see that function).
async fn under_agent_cap(notify: &NotifyDeps, user_id: &str) -> Result<bool, sqlx::Error> {
    let (count,): (i64,) = sqlx::query_as(
        "select count(*) from notifications \
         where user_id = $1::uuid and kind = 'mention' and created_at > now() - interval '1 day'",
    )
    .bind(user_id)
    .fetch_one(&notify.pg)
    .await?;
    if count >= AGENT_MENTION_DAILY_CAP {
        tracing::warn!(
            "[mentions] daily cap of {AGENT_MENTION_DAILY_CAP} agent mentions reached for {user_id}; dropped"
        );
        return Ok(false);
    }
    Ok(true)
}

async fn notify_these(
    notify: &NotifyDeps,
    to: &[&Mentionee],
    sender: &MentionSender<'_>,
    content: &str,
    where_: &str,
    href: &str,
    opts: &MentionOpts,
) -> Vec<String> {
    let mut told = Vec::new();
    for m in to {
        let body = if content.chars().count() > 200 {
            let clipped: String = content.chars().take(200).collect();
            format!("{clipped}…")
        } else {
            content.to_string()
        };
        let title = format!("{} mentioned you in {where_}", sender.label);
        let input = NotificationInput {
            kind: "mention",
            title: &title,
            body: Some(&body),
            href: Some(href),
        };
        let written = if sender.user_id.is_empty() {
            match under_agent_cap(notify, &m.user_id).await {
                Ok(true) => add_notification_with(
                    notify,
                    &m.user_id,
                    &input,
                    &mention_meta(sender, opts, &m.user_id),
                )
                .await
                .map(|_| true),
                other => other,
            }
        } else {
            add_notification_with(
                notify,
                &m.user_id,
                &input,
                &mention_meta(sender, opts, &m.user_id),
            )
            .await
            .map(|_| true)
        };
        match written {
            Ok(true) => told.push(m.user_id.clone()),
            Ok(false) => {}
            Err(e) => tracing::error!("[mentions] notification for {} failed: {e}", m.user_id),
        }
    }
    told
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
    fn mentioned_ids_are_who_notify_mentions_tells() {
        let members = [
            member("a", "Ada Lovelace", "ada@x.test"),
            member("c", "Cy Young", "cy@x.test"),
            member("s", "Sam Sender", "sam@x.test"),
        ];
        assert_eq!(mentioned_ids(&members, "s", "@ada and @sam, look"), ["a"]);
        assert!(mentioned_ids(&members, "s", "no mentions").is_empty());
        // An agent sender (no user id) excludes nobody.
        assert_eq!(mentioned_ids(&members, "", "@sam"), ["s"]);
    }

    #[test]
    fn a_mention_row_names_its_sender_and_obeys_the_mute() {
        let opts = MentionOpts {
            entity: Some(NotifyEntity::new("task", "t-1")),
            quiet: ["muted".to_string()].into_iter().collect(),
        };
        let person = MentionSender::of("u-1", "Ada");
        let m = mention_meta(&person, &opts, "muted");
        assert!(m.quiet && m.group_key.is_none());
        assert_eq!(m.entity, Some(NotifyEntity::new("task", "t-1")));
        let a = m.actor.unwrap();
        assert_eq!((a.kind, a.id.as_str()), (ActorKind::User, "u-1"));
        assert!(!mention_meta(&person, &opts, "other").quiet);
        let agent = MentionSender {
            user_id: "",
            label: "Margaret",
            agent_model: Some("margaret-marketing"),
        };
        let a = mention_meta(&agent, &opts, "x").actor.unwrap();
        assert_eq!(
            (a.kind, a.id.as_str(), a.label.as_str()),
            (ActorKind::Agent, "margaret-marketing", "Margaret")
        );
        // An agent known only by its label still names someone.
        let a = MentionSender::of("", "Dex").actor();
        assert_eq!((a.kind, a.id.as_str()), (ActorKind::Agent, "Dex"));
    }

    #[test]
    fn token_membership_matches_dashed_and_localpart_forms() {
        let tokens = user_mention_tokens(Some("Jon Iler"), Some("jon@x.com"));
        for t in ["jon", "jon-iler"] {
            assert!(tokens.contains(&t.to_string()), "missing {t}");
        }
    }
}
