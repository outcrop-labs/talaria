// Channels, messages, reactions, plans, DMs, chat and conversations.
pub mod channels;
pub mod channels_id;
pub mod channels_id_agents;
pub mod channels_id_conclude;
pub mod channels_id_events;
pub mod channels_id_members;
pub mod channels_id_messages;
pub mod channels_id_messages_msgid;
pub mod channels_id_messages_msgid_reactions;
pub mod channels_id_plan;
pub mod channels_id_read;
pub mod channels_id_teams;
pub mod chat;
pub mod chips;
pub mod chips_approvals_id;
pub mod chips_expose;
pub mod conversations;
pub mod conversations_id;
pub mod conversations_id_read;
pub mod dms;

use talaria_channels::channel_role;
use talaria_state::AppState;

/// What a channel route needs of the caller. Both answers come from the same
/// read, which is why six route files each wrote the same gate out by hand.
#[derive(Clone, Copy)]
pub(crate) enum ChannelNeed {
    Member,
    Owner,
}

/// The channel gate: `Member` is "any role at all", `Owner` narrows it. A
/// failed role read DENIES — the safe direction for a gate — and logs under
/// the domain tag; `where_` is the middle of that sentence, so each route
/// keeps the log line it had.
pub(crate) async fn channel_gate(
    state: &AppState,
    user_id: &str,
    id: &str,
    need: ChannelNeed,
    where_: &str,
) -> bool {
    match channel_role(&state.pg, user_id, id).await {
        Ok(Some(role)) => match need {
            ChannelNeed::Member => true,
            ChannelNeed::Owner => role == "owner",
        },
        Ok(None) => false,
        Err(e) => {
            tracing::error!("[channels] role read{where_} failed: {e}");
            false
        }
    }
}
