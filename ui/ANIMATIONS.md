# Animation pass — the motion contract

One motion language for the whole cockpit, built ONLY from native Svelte
primitives: `svelte/transition` + `svelte/animate` via the reduced-motion-aware
wrappers in `@/lib/motion` (`fade`, `fly`, `scale`, `slide`, `flip`, presets
`QUICK`, `POP`, `PANEL`, `LIST`). Import from `@/lib/motion` — never directly
from `svelte/transition`/`svelte/animate`, so reduced-motion stays handled in
exactly one place. No CSS keyframe additions, no JS animation libraries.

Mercury is matte and calm (spec §9): motion confirms what happened; it never
performs. If a transition would make someone wait, it's too long. If it would
make them look, it's too loud.

## The grammar

| Surface | Enter | Exit | Notes |
|---|---|---|---|
| Modal / dialog | `in:scale={POP}` | `out:fade={QUICK}` | Modal.svelte already does this — reuse Modal, don't re-animate its children |
| Dropdown / context menu / popover / tooltip / suggest list | `in:scale={{ ...POP, start: 0.97 }}` | `out:fade={QUICK}` | set `transform-origin` toward the trigger (e.g. `origin-top-left`) via class |
| Side panel / drawer | `in:fly={PANEL}` (x or y toward its resting place, 8–16px) | `out:fade={QUICK}` | |
| Banner / inline notice / error row | `transition:slide={{ duration: 150 }}` | same | height animation announces "something appeared between things" |
| Collapsible / disclosure | `transition:slide={{ duration: 150 }}` | same | |
| List rows appearing/disappearing (chat messages, notifications, activity, table rows) | `in:fade={{ duration: 150 }}` | `out:fade={QUICK}` — or `out:slide` when siblings should close the gap smoothly | keep it cheap; no travel on rows |
| Keyed list REORDER / cross-column move (kanban cards, sortable rows) | `animate:flip={LIST}` on the `{#each}` item | — | flip only where order genuinely changes while visible |
| Empty state / zero-state swap | `in:fade={{ duration: 150 }}` | none | |
| Toast / floating status | `in:fly={{ y: 8, duration: 180 }}` | `out:fade={QUICK}` | |
| Wizard / step change | `in:fade={{ duration: 150, delay: 80 }}` on the entering step | `out:fade={QUICK}` | delay lets the leaving step clear; no horizontal slides |

## Hard rules

- **Never animate**: route/page swaps, the nav rail's contents on load, large
  scroll containers, text mid-reflow, anything on a hot path that streams
  (message tokens during SSE, generating indicators beyond what exists).
- **No loops, no springs, no bounce, no stagger cascades.** Durations 120–200ms;
  exits ≤ enters.
- **`{#if}` + `transition:` is the pattern** — never mount/unmount via CSS
  visibility to fake it.
- Svelte 5 transitions are LOCAL by default (they don't fire when a parent
  mounts) — that's what we want. Do not add `|global`.
- A transition on a `{#each}` row must not fire for the initial page render's
  rows — local default handles this; verify by loading the page fresh.
- Don't double-animate: if a parent already transitions (Modal), children don't.
- Don't touch `animate:flip` onto lists that can exceed ~100 visible items.
- Skeletons already pulse via CSS and stagger via their `delay` prop — leave
  the skeleton system alone.
- If a surface already has a transition from the migration, keep it unless it
  breaks this grammar.

## Definition of done per file

- Only `@/lib/motion` imports added (plus preset consts); Tailwind classes
  otherwise untouched except `origin-*` where a popover needs it.
- The dev server (already running) compiles it — check for red in
  `npx svelte-check` on YOUR files if unsure.
- A one-line comment is NOT needed for each transition — the grammar is the
  documentation. Comment only where you deviate and say why.
