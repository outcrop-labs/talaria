- **Universal @mentions (#60).** Coverage holes closed so a mention behaves
  the same no matter who writes it or where. Agent-authored mentions now
  notify: an agent posting to a channel (the agent-key write path returned
  before the notify call — "@jon can you check this" from an agent reached
  nobody), a streamed agent channel reply, and an agent's plan turn all
  notify @mentioned humans exactly like human messages. Ticket
  DESCRIPTIONS notify board members on change, mirroring the comment
  contract. Plan mentions got three fixes: the composer now suggests the
  plan's actual members instead of the whole org (mentioning someone who
  couldn't read the plan silently notified nobody), the notification
  deep-links to `/plan?p=…` instead of `/artifacts`, and a plan whose doc
  doesn't exist yet falls back to plan membership as the read boundary.
  And mentions are finally VISIBLE: a shared remark pass in the Markdown
  renderer highlights @tokens on every surface at once (chat, channels,
  comments, descriptions, plan turns) — code spans untouched, emails don't
  match. Still recorded as future threads: the TipTap @-suggestion (rich
  composers), research/KB mention eligibility.

### Changed
