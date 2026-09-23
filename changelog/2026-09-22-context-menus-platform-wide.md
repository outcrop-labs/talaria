- **Context menus, platform-wide.** Right-click is real now: one primitive
  (`ui/context-menu.tsx` — cursor-positioned, keyboard-navigable, portaled,
  viewport-clamped) wired onto every row, card, and tile: kanban cards and
  board list rows (open / copy link / copy ticket ref / archive), nav board
  rows, channel/relay/DM rows (open / copy link / mark-read that reads the
  REAL cursor), agent thread rows, message bubbles (copy text), knowledge
  spaces and docs, artifacts and folders (incl. copy PUBLIC link when a
  slug exists), the agents roster (the full lifecycle cluster, confirm
  texts shared with the buttons so they can't drift), research runs, and
  every home console row. Every item calls the exact function its button
  counterpart calls — a context menu is a shortcut, never the only home of
  an action (the rule lives in UI-CONVENTIONS). Native browser menus are
  suppressed app-wide EXCEPT on editable fields, where paste and
  spellcheck are real workflows.

### Changed
