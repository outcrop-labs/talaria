- **The inbox command's action selection and the research scoper's `crisp`
  verdict are now measured against a typed judgment** — and the reason both are
  *measured* rather than switched over is the same, which is worth stating once
  about the class rather than twice about the instances.

  **THE VERDICT IS COUPLED TO ITS PROSE.** The inbox command returns
  `{message, actionId}` and a `CommandTurn`'s `payload` belongs to its
  `action_id` — a `reply` carries the exact text to post — so swapping the id
  without swapping the payload produces a proposal that does not match its own
  arguments. The scoper returns `{crisp, read, questions}` where `questions` is
  non-empty *exactly* when `crisp` is false, and both halves are posted into
  the run's discussion for a person to read — so overriding `crisp` to true
  would discard questions somebody was about to be asked, and to false would
  produce a vague verdict with nothing to ask. In both cases the typed answer
  cannot replace the verdict without also replacing the prose that belongs to
  it. That is a two-step restructure — a typed primitive for the verdict, a
  text model for the half the chosen branch needs — and it is worth doing once
  the ledger says the judgment is right often enough to pay for it. Measuring
  first is not caution here; it is the only order in which the acting version
  can be written correctly.

  **One correction to the plan, and it changes what the inbox site is for.**
  The audit described it as "a Choice over the allowed ids", assuming several.
  `allowed_focus_action_ids` returns the card's whole action list only for a
  **widened** model; otherwise it returns at most one id — the deterministic
  proposal's — and in plan mode none at all. A Choice over one option is not a
  question and the port refuses it. So the real question was never "which of
  these several" but **"this one, or none"**: whether the instruction asks for
  the authorized action at all. `none` is always offered, which makes a
  one-element allowlist a real question and makes "propose nothing" something
  the model can *say* rather than something inferred from a low probability —
  the same shape gap alignment already uses.

  What the Choice does buy, even unacted-on: its **options are the allowlist**,
  so it cannot name an id outside it. That is precisely what
  `validate_command_object` and a repair turn currently spend effort proving
  about a JSON string. The authority gate is untouched and still decides what
  may be proposed; this is a measurement of how often it has anything to
  reject.

  The scoper's question carries the **depth**, because crispness is
  mode-relative: "compare our two options" is a crisp recon and an
  underspecified expedition, and a judgment that cannot see the mode is
  answering a different question than the harness was asked.

  Both sites' baselines render in the comparator's own vocabulary — a null
  `actionId` and the Choice's `none` are the same answer and must spell
  identically, and `crisp` renders as the exact `"true"`/`"false"` that
  `noul_agrees` reads — because two ledger columns that mean slightly different
  things produce an agreement rate that is noise. Both passes are detached and
  run after the answer is already decided, so neither can change what its
  caller returns.

  Verified: 92 tests in `talaria-decide` (7 new — one authorized action plus
  `none` is a well-formed Choice while a bare allowlist of one is not, nothing
  is asked with an empty instruction or an empty allowlist, the two columns
  share one vocabulary including both-decline being agreement, a noul can never
  agree at a Choice site, the scoper's baseline is the exact spelling
  `noul_agrees` compares against, the depth is in the state, and both censuses
  say "Nothing yet" so the panel cannot promise behaviour the site does not
  have). clippy clean across `talaria-decide`, `talaria-inbox-focus` and
  `talaria-research-def`; `bun run gate` green.

  **Neither has run against a real provider.** Both floors are what the ledger
  will be read against rather than anything currently in force, since neither
  site acts.
