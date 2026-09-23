- **W4e — two menu builders and one sort header.** `openCopyItems(path, open,
  icons?)` and `copyTextItems(value)` join `components/ui/context-menu.svelte.ts`
  (the real home; the plan pointed at `ui/src/`) and absorb ~17 hand-built
  "Open / Copy link" and "Copy text" pairs across boards, comms, knowledge,
  research, teams, templates, the four home tabs, the KB editors and the chat
  message menu — each site keeping its own labels, hrefs and icons.
  `components/ui/SortHeader.svelte` renders ONLY the sortable `<button>`: the
  label comes in as children and the table's own geometry classes through
  `class`, so every `<th>` and grid cell keeps its markup. Both sort cycles
  (BoardList's `flex-row-reverse` chevron pair, ArtifactsBrowser's 10px arrow
  pair) keep their glyphs and order.
