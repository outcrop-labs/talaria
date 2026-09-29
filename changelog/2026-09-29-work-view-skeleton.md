- **Work: a new view for working alongside an agent on a document.** Below
  Comms in the nav, `/work` lists your work sessions per agent and opens one
  at `/work/<id>` — a linkable, back/forward-able address, the same shape Plan
  and Research use. A session is a conversation with `kind='work'`, so it
  inherits the Plan idiom whole rather than growing a parallel one: owner and
  collaborators, team sharing, per-turn author labels, presence, the read
  cursor and the unread count all already understand it. The stage is two-pane
  from the first keystroke — the session's stream on the left, the document
  column on the right — so the pane that fills that column later does not
  re-lay the surface out around it. Sessions rename, archive, restore and
  delete from the header and the row menu, owner-only for the destructive
  three; deleting a session says plainly that files it opened are untouched.
  Starting one needs the new **Start work sessions** permission (on by default
  for members, in the Work group beside Create plans), and admins can grant or
  revoke the whole view per user or per team like any other work view.

  Under the hood this is a widening, not a new engine: one appended migration
  adds `conversations.pinned_files` (the document pane's per-session memory,
  defaulting to `[]` so nothing needs a backfill), and the conversation access
  gates, the list, the unread total and `/api/chat` all learned `'work'`
  alongside `'plan'`. The rail is the plan rail with its noun, its `+`
  permission and its tooltip as props rather than a second copy, and the chat
  surface's four hand-written copies of the kind union became one exported
  `ChatKind`. `useHasPerm` now accepts a getter, the same `MaybeGetter` every
  other hook in `lib/` takes, so a component can ask about a permission it
  received as a prop without reading it once at init.

  This is the skeleton. Sharing controls and presence, the document pane
  itself (native artifacts, then the editable Google embed) and inline
  approval cards in the stream are the phases after it.

  Verified: `bun run gate` green — `check`, `svelte-check` 0 errors (2
  pre-existing a11y warnings in WorkchainCanvas, untouched), 1321 ui tests
  (83 files), and fmt/clippy/tests for the four api packages the diff
  touches. Schema snapshot regenerated; its diff is the single `pinned_files`
  column. Exercised against a running stack with a real session:
  `?kind=work` lists only work sessions and `?kind=chat` only chats (neither
  leaks into the other), an unknown kind still falls back to chats, a work
  session's own `GET /api/conversations/<id>` answers 200 where the
  unwidened gate would have 404'd, and a new row's `pinned_files` defaults
  to `[]`.
