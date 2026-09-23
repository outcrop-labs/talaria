- **W8a — QueryState adoption, 24 of the surfaces that hand-rolled it.** The
  plan listed 19 files; the agent adopted **24** forks across 28 listed surfaces
  and deliberately left four, each for a reason the parity rule covers:
  * `Comms.svelte` keeps its rail markers. Its failure row is
    `<QueryError variant="inline">` inside `px-2 py-1.5`, matched to `RailRow`'s
    own padding, and QueryState's error branch hard-wraps its QueryError in
    `<div class="h-full">` with no class hook — swapping would move every rail
    failure row 8px left and 6px up of the rows it replaces, which is exactly
    what `RailFailure`'s comment says it exists to prevent. Its channel-detail
    pill fork has NO error arm, so adopting would newly let a settled 500 drop a
    centred error into the 48px channel header — care point 1 forbids it. (This
    corrects an earlier note of mine: no concurrent write was lost on this file;
    `Comms.svelte` was a deliberate non-adoption, and W4e's `openCopyItems` call
    sites are intact. I verified the same for every other file two slices
    touched — KbDocEditor, KbSpaceEditor, ChatView, `board-list.ts`,
    `BoardsSublist`, `ArtifactEditor` — all kept both edits.)
  * The three undefined-branch decisions are recorded per file: `idle={null}`
    where the hand-rolled code shimmered, the `empty` slot where it had a
    distinct empty state (`AdminOrgGooglePanel`, `AdminInstanceDomainPanel`,
    `AdminInvitesPanel`, `IntegrationsSection`, `AssistantGoogleCard`), and the
    default idle line where it already said "nothing selected".
  Typecheck is green (0 errors, 138 pre-existing warnings). One correction worth
  keeping: five of the "unused `data`" hits were on a dead script-level
  `const data = $derived(query.data)`, not on the snippet params — the fix was to
  delete the dead derived, not to rename the param, which would have left the
  body reading an `X | undefined`.
