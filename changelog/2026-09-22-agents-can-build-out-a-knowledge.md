- **Agents can build out a knowledge space, not just append to it.**
  Reported live from a fleet deploy: an agent asked to put a space's intro
  and table of contents on the space page itself had no tool that could
  reach it — `create_kb_space` writes a landing body only at creation, and
  nothing after can edit it — so the content landed in a lookalike
  top-level doc, and with no move or delete the misfile couldn't be
  corrected, only duplicated. Three tools close the gap. `edit_kb_space`
  (name, description, icon, and the landing markdown; admission mirrors
  doc edits — authorship, an editor grant, or an elevated assistant on
  non-private material — and sharing fields stay human-only).
  `move_kb_doc` (nest under a same-space parent, lift to the space's top
  level, or reorder among siblings), under the same edit admission the
  doc's PUT already grants. And `delete_kb_doc`, deliberately narrower
  than the edit beside it: an edit is versioned and recoverable, a delete
  is not, so an agent deletes only docs it created and re-files anything
  else (`move_kb_doc` re-files without destroying). Doc deletes now land
  in the audit log whoever pulled the trigger. All three are mirrored
  through the fitness lockstep — catalog, sandbox backends, exercise
  tests — so the toolkit the next agent is trained against knows them.
