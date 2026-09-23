- **The Combobox walks its list by keyboard.** `Combobox.svelte` had no
  arrow-key navigation — the search input handled only Escape (and, in the
  allowCreate tag-input mode, Enter/comma), so every other pick was
  mouse-only. Now ↑/↓ walk a highlight through the filtered rows (wrapping,
  skipping the Create row), Home/End jump to first/last, Enter activates the
  highlighted row (toggles in multi mode, selects-and-closes in single mode),
  Tab closes without activating, and ArrowDown/ArrowUp on a closed combobox
  opens it. The highlight re-enters at the first filtered row on open and on
  every keystroke, draws a `bg-hover` fill, and scrolls into view. One latent
  gap fixed on the way in: Svelte's native `autofocus` only fires when body
  holds focus, so a trigger CLICK left focus on the button and typing went
  nowhere — the panel now focuses the search field explicitly on open. The
  listbox story is also honest now: `aria-haspopup`/`aria-expanded` on the
  trigger, `role="listbox"`/`role="option"` + `aria-selected` on the rows,
  and `aria-activedescendant` tracking the highlight. Mouse behavior is
  unchanged — click still toggles.

  Verified: `bun run verify` green — svelte-check 0 errors over 5402 files,
  1270 UI tests, invariants and changelog checks. Exercised the mounted
  component in the browser: open → highlight on first row, ↓×12 on a 30-row
  list scrolls the highlight into view (scrollTop 0→186, row in view at every
  step), End→Home jump and scroll back, Enter toggles a row on and back off
  through a real `$state` parent (events `['opt0','opt0,opt1','opt1']`),
  Escape closes and returns focus to the trigger, Tab closes with no event
  fired, click path still toggles, and single-mode Enter selects and stays
  closed. The stale-focus bug was reproduced first: before the focus fix,
  typing "br" after opening by click never reached the input.