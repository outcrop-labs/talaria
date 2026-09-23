- **A platform-default reasoning effort, yours.** Settings → Preferred model
  now offers a "Default reasoning effort" pick directly beneath the model —
  but only when the selected model publishes effort levels (a model with no
  ladder shows no control; there is nothing to default). The saved level
  becomes the starting pick everywhere effort is offered — Comms agent chats
  (tiers included) and the assistant panel — wherever the model in play
  supports it; models that don't simply run at their own default and show no
  chip. An explicit pick in any conversation (including auto) stays
  authoritative for that conversation, and the default re-seeds exactly when a
  model switch retires the pick. Stored on the profile beside the preferred
  model (`users.preferred_effort`); picking auto clears it. Because the
  preference travels across models, the server stores the bare level string
  and each surface applies it only against the levels that model's metadata
  vouches for — a stale level is inert, never an error.
