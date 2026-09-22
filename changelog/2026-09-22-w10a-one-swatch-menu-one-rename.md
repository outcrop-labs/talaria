- **W10a — one swatch menu, one rename field, one add row.**
  `components/ui/{ColorsMenu,RenameField,AddRow}.svelte` (−98 lines across the
  two settings tabs and the ticket menu). **The add row is `AddRow`, not the
  planned `InlineCreate`**: `InlineCreate.svelte` already exists as a committed,
  SDK-exported primitive (a bare `+` that expands), and adopting it would have
  changed the tabs' always-open "New status + Add" look — the parity rule's
  case, so the row was extracted under its own name. The three components own
  no writes: the tabs keep their `run()`/invalidation and `confirm()`/refresh
  paths, and `AddRow` preserves the one behavioural difference between them —
  StatusesTab clears the draft immediately (even on failure), LabelsTab only
  after `.then(refresh)` — by clearing once the submitted promise settles.
