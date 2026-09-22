- **Chats stopped 500ing — every conversation read, on every instance, since
  the rooms cutover.** The cutover's migration said "every reader of
  `tasks.conversation_id` was re-pointed before this drop," and six readers
  were not: the conversation access predicates, the prior-turn transcript,
  the notification audiences, and the task-delete unindex all still joined
  the dropped column. Postgres validates a statement's whole text when it
  plans it, so the mere presence of the dead reference broke EVERY
  conversation detail and history read — ticket branch or not, chat or plan
  or research — which is why a threads sidebar looked fine while opening
  any thread answered 500. The dead legs are gone (no `kind='ticket'`
  conversation can exist since the cutover deleted them all; a task's
  thread is a channel room, gated by board membership on the channels
  side), and the delete-task unindex is re-pointed to the room's
  `channel_messages` — since the cutover it errored silently, leaving a
  deleted ticket's comments answering searches. Verified against a
  post-cutover database: the detail route answered 500 before the fix and
  200 after, same row, same session.
