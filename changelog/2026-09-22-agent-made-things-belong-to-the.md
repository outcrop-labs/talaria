- **Agent-made things belong to the human behind the run — the responsible
  user, not the agent.** A knowledge doc or space, a document, file, or saved
  image, a research report, a workbench plan: whatever an agent creates now
  carries an owner, resolved by an attribution ladder — a personal
  assistant's owner; the human an org agent is answering mid-chat (the same
  turn key research reports back to); otherwise the admin who hired the
  agent; and an untraceable agent stays ownerless, as before. The owner gets
  the ordinary human's controls — share, make private, set policy — replacing
  the old answer where org-agent output was locked to an allow-list nobody
  could sit inside. Default visibility is deliberately unchanged: an org
  agent's output stays workspace-readable (ownership adds control, not
  secrecy), a personal assistant's stays private to its owner, and an org
  agent's research stays org-visible and ambient-indexed however owned —
  org-ness is derived from the run's own row (`requested_by` = the model,
  plus the agent being ownerless in `agent_defs`), never a serialized flag,
  so runs in flight across the deploy resolve correctly. The artifact
  listing also caught up with the single-artifact GET: a personal assistant
  now sees its owner's private files in the Files browser it could already
  open them from. Going forward only — existing ownerless items stay as they
  are.
