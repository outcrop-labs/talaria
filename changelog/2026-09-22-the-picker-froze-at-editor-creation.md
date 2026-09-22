- **The @ picker froze at editor creation — and ticket threads never
  offered agents.** MentionSuggest captured the mentionables array at
  mount, but every surface builds that list with `$derived` over async
  queries: the picker showed whoever had resolved by editor creation
  (cached users, typically) and never the agents that land with the
  fleet — and a list still empty at mount installed no picker at all,
  leaving the @ key dead. Candidates are read live on every keystroke
  now, and a ticket's discussion offers the board's agents ahead of its
  members, addressed by label — the same grammar the channels read. The
  description editor stays humans-only: a mention there notifies a
  person, and no agent reads a description.
