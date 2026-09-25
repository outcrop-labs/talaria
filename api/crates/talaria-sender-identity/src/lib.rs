// THE PER-TURN SENDER-IDENTITY MESSAGE. TALA-80: a chat agent with persistent
// memory will happily fill an unattributed user turn with whoever its memory
// names — the remembered user, a profile from weeks ago, a person from a
// different conversation — and answer that person instead of the one actually
// typing. The turn alone cannot fix this, because the turn is just text; the
// identity has to arrive WITH the turn, stamped onto the outgoing messages
// array by the door itself, sourced server-side from the authenticated row
// rather than from anything the client or the model could get wrong. Every
// human-to-agent chat door shares this one builder so the sentence every door
// stamps is the same sentence.

/// Who is speaking this turn, as the authenticated row says. The id is the
/// only field the door always has; name and email are the display facts the
/// row may or may not carry.
#[derive(Debug, Clone)]
pub struct SenderIdentity {
    pub id: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

impl SenderIdentity {
    /// The one string the prompt calls this person by: the name if the row
    /// has one, else the email, else the honest placeholder. Never invents a
    /// name — a fabricated "the user" is exactly the ambiguity TALA-80 is
    /// about.
    pub fn display(&self) -> String {
        match (&self.name, &self.email) {
            (Some(name), _) => name.clone(),
            (None, Some(email)) => email.clone(),
            (None, None) => "an unidentified user".to_string(),
        }
    }
}

/// The clause that does the actual work: address the live speaker by name,
/// and treat any remembered profile that names someone else as describing a
/// different human. Constant text — the wording is the contract, so it is
/// shared verbatim by every door rather than re-phrased per call site.
pub const SENDER_IDENTITY_CLAUSE: &str = "Address THEM by name. If your remembered user profile or memory names a different person, that memory describes a different human in a different conversation — never attribute it to the person speaking now.";

/// The block stamped onto the turn: who is speaking, their id, and the
/// clause. Pure — takes the identity, not the pool — so the door pays for
/// one read and the block is testable without a database.
pub fn sender_identity_block(sender: &SenderIdentity) -> String {
    format!(
        "WHO YOU ARE SPEAKING WITH: this turn's live sender is {} ({}). {}",
        sender.display(),
        sender.id,
        SENDER_IDENTITY_CLAUSE
    )
}

/// The block as a messages-array entry: a system message, so the identity
/// rides with the turn in the same shape every door already speaks.
pub fn sender_identity_message(sender: &SenderIdentity) -> serde_json::Value {
    serde_json::json!({
        "role": "system",
        "content": sender_identity_block(sender),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sender(name: Option<&str>, email: Option<&str>) -> SenderIdentity {
        SenderIdentity {
            id: "user-1".to_string(),
            name: name.map(str::to_string),
            email: email.map(str::to_string),
        }
    }

    #[test]
    fn the_name_wins_over_the_email() {
        assert_eq!(
            sender(Some("Ada"), Some("ada@example.com")).display(),
            "Ada"
        );
    }

    #[test]
    fn the_email_wins_when_there_is_no_name() {
        assert_eq!(
            sender(None, Some("ada@example.com")).display(),
            "ada@example.com"
        );
    }

    #[test]
    fn an_unidentified_user_when_there_is_neither() {
        assert_eq!(sender(None, None).display(), "an unidentified user");
    }

    #[test]
    fn the_block_carries_display_id_and_clause() {
        let block = sender_identity_block(&sender(Some("Ada"), Some("ada@example.com")));
        assert!(block.contains("Ada"));
        assert!(block.contains("user-1"));
        assert!(block.contains(SENDER_IDENTITY_CLAUSE));
    }

    #[test]
    fn the_message_is_a_system_message_with_the_block_as_content() {
        let message = sender_identity_message(&sender(Some("Ada"), None));
        assert_eq!(message["role"], "system");
        assert_eq!(
            message["content"],
            sender_identity_block(&sender(Some("Ada"), None))
        );
    }
}
