# Animation pass — the motion contract (round 2)

One motion language for the whole cockpit, built ONLY from native Svelte
primitives (`svelte/transition`, `svelte/animate`, actions + WAAPI) via the
reduced-motion-aware wrappers in `@/lib/motion`. Import from `@/lib/motion` —
never directly from `svelte/transition`/`svelte/animate` — so reduced-motion
stays handled in one place. No animation libraries, no CSS keyframe additions
outside the sanctioned view-transition block in styles.css.

Round 2 verdict (design review): the first pass was too timid — 160ms at 0.96
scale reads as *nothing*, and unanimated container resizes read as jank. The
grammar now is: **perceptible entrances, animated resizes, staggered content,
transitions between views and tabs.** Still matte, still calm — motion confirms,
never performs — but it must be *felt*.

## Primitives (`@/lib/motion`)

- `pop` — overlay entrance: rise + scale + fade, one quint curve. Modals
  (`in:pop`), popovers (`in:pop={POPOVER}`).
- `fade`, `fly`, `scale`, `slide` — as before; presets carry quint easing.
- `flip` + `LIST` — keyed-list reorder/move.
- `autoHeight` action / `<AutoHeight>` (ui) — the box glides to its new height
  whenever content height changes. **Rule: if it resizes, it animates.**
- `staggerIn` action — direct children rise in 40ms apart (WAAPI,
  `fill: backwards`, `data-no-stagger` to opt an element out). Put it on the
  container INSIDE a `{#key …}` block so step/tab changes re-run it.
- `markCrossfade()` — send/receive pair for the one element that moves between
  spots (tab underline, segmented thumb).
- Presets: `QUICK` (exit fade 140), `POP`, `POPOVER`, `PANEL` (y:14/240),
  `PANEL_X` (x:16/240), `LIST` (180).

## The grammar

| Surface | Treatment |
|---|---|
| Modal / dialog | `in:pop` / `out:fade={QUICK}`; backdrop `in:fade={{duration:180}}` / `out:fade={QUICK}` (Modal.svelte does this — reuse it, don't re-animate children) |
| Dropdown / menu / popover / tooltip / suggest | `in:pop={POPOVER}` / `out:fade={QUICK}`, `origin-*` toward the trigger |
| Side panel / drawer — OVERLAY (portaled, out of flow) | `in:fly={PANEL}` or `={PANEL_X}` toward resting place / `out:fade={QUICK}` (Drawer.svelte) |
| Side panel — IN-FLOW (occupies layout: thread panel, comment rails, history rails) | `transition:slide={GROW_X}` on BOTH legs (`|global` when the panel is the component root) — the panel grows open and shrinks closed so siblings glide instead of snapping. If text visibly reflows during the grow, pin an inner wrapper to the panel's resting width so it clips instead |
| Wizard / step flow | `{#key step}` → container with `use:staggerIn`, wrapped in `<AutoHeight>`; no per-branch fade soup, the stagger IS the entrance |
| Tab pane change | `{#key active}` → pane `in:fly={{ y: 6, duration: 200 }}` (no exit — the new pane replaces in place); `use:staggerIn` where the pane is section-shaped; `<AutoHeight>` when the pane's container height varies (modals, settings). Indicator/thumb moves via `markCrossfade` pair |
| View change (nav) | `data-view-transition` on the nav link — the View Transitions API cross-fades ONLY the `.vt-view` region (CSS in styles.css). Never animate the rail/strip |
| Page content entrance | `use:staggerIn` on the view's top-level section container — the page's panels/sections rise in as the view mounts (composes with the view transition), and again when loaded content replaces a skeleton (put it on the loaded branch's container). A section whose meat is a list carries `data-stagger-items` (optionally `="<selector>"`) so its rows cascade inside the section's slot |
| **Any grid or list** (rule of thumb) | `use:listStagger` on the `{#each}` container — items rise subtly, 30ms cadence, capped at 12 slots so big tables never crawl. Inside a `use:staggerIn` region, prefer `data-stagger-items` on the section; a standalone list that also sits under staggerIn adds `data-no-stagger` to itself so exactly one cascade owns it. Exempt: chat transcript history, virtualized/streaming lists, skeleton branches |
| Banner / notice / error row | `transition:slide={{ duration: 150 }}` |
| Collapsible / disclosure | `transition:slide={{ duration: 150 }}`; if a sibling container visibly resizes with it, that container rides `<AutoHeight>` |
| List rows in/out | `in:fade={{ duration: 150 }}` / `out:fade={QUICK}` (or `out:slide` when siblings should close the gap) |
| Keyed reorder / cross-column move | `animate:flip={LIST}` |
| Empty/error state swap | `in:fade={{ duration: 150 }}` |
| Skeleton → content | `<Materialize>` (ui): item-SHAPED skeletons (a snippet mirroring the real item's silhouette — frame, avatar block, line widths) render in the same container/geometry as the list, then fade out in place while the items stagger in over them. Grid-stacked, so no layout jump. Generic `SkeletonRows` stays only for non-list prose regions. The skeletons' own material is SIGNAL STATIC (`lib/skeleton-static.ts`) — one page-wide dither field on a shared 8Hz ticker, with no per-block delay: the cascade belongs to the CONTENT arriving, never to the waiting |
| Confirmation flash (saved/copied) | `in:fade={{ duration: 150 }}` |

## The `|global` rule (round 3 — this bug shipped twice)

Svelte transitions are LOCAL by default: they play only when their own block
toggles, and are SUPPRESSED when the block is created because an ancestor
mounted. Most overlays are rendered `{#if x}<SomeModal …>` — the component
mounts with its internal `{#if open}` already true, so a local intro never
plays. That is how "modals are completely unanimated" survived two review
rounds.

- **Overlay roots** (modal, drawer, side panel, popover panel — anything that
  should animate *no matter what mounted it*): every transition leg gets
  `|global` (`in:pop|global`, `out:fade|global={QUICK}`).
- **Rows, banners, inline reveals** (things that must NOT animate on page
  load): stay local — that's what local is for.
- Litmus test: "should this animate when its parent component mounts?" Yes →
  `|global`. No → local.

## Hard rules

- **Never animate**: streaming token text, live readouts on data change,
  skeletons, initial page render (local rows cover it).
- **No loops, no springs, no bounce.** Stagger is sanctioned ONLY via
  `staggerIn` (uniform 40ms cadence) — no hand-rolled per-element delays.
- Durations 140–240ms; exits ≤ enters; quint ease via the presets.
- `{#if}` + `transition:` is the pattern — no CSS-visibility fakes.
- Don't double-animate: Modal's children, staggered containers' grandchildren.
- A resize the user watches must glide (`AutoHeight`); a resize behind a
  view-transition or a full swap must not double-animate.
- Any element that opens an INLINE route (a child route mounting inside the
  current view — ticket modal et al) carries `use:warmRoute` (`@/lib/warm-route`)
  with the route path + a data prefetch: a cold chunk's first evaluation
  blocks the main thread through the overlay's entrance window, so the first
  open renders with its animation already elapsed.

## Definition of done per file

- Only `@/lib/motion` / `<AutoHeight>` imports added; Tailwind classes
  untouched except `origin-*` where needed.
- svelte-check clean; comment only where you deviate, and say why.
