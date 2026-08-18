# Talaria UI conventions

The contract that keeps fifteen surfaces feeling like one app. Written after a
full audit (2026-07-10, screenshots + code inventory); enforced by shared
primitives — reach for the primitive, not a hand-rolled recreation.

## Page shells — exactly two archetypes

1. **Rail surface** (work surfaces: Comms, Plan, Research, Knowledge,
   Artifacts): `<RailSurface>` → `<Rail>` (LEFT, always `w-72`, `bg-sidebar`,
   `border-r`) + `<Stage header={<StageHeader …>}>`. The rail header and stage
   header are both `h-12`, so the top line runs straight across the app.
   Never put a rail on the right; never invent a new width.
2. **Page surface** (Home, Agents, Models, MCP, Observability, Studio,
   Templates, Apps, Admin, Settings): `<PageSurface>` — it owns the scroll box,
   the `p-8` gutter and the centred column. Put section rhythm (`space-y-6`) and
   `use:staggerIn` on your own child inside it. Title row: `<ViewHeader>`.

Components: `components/app/` — `PageSurface`, `RailSurface`, `Rail`, `Stage`,
`StageHeader`, `RailSection`, `RailRow`, `CountPill`.

### One width scale, four names

Every centred column in the app is one of three tokens (`styles.css`). They are
tokens, not Tailwind classes at the call site, because that is where this broke
before: page surfaces alone had drifted to `max-w-2xl` / `4xl` / `5xl` / `6xl`,
`Models` changed width between its own TABS, and the `AppLayout` skeleton was
wider than most of the content it stood in for — so a cold load settled narrower
and every tab change slid sideways.

| Token | Width | Used by |
|---|---|---|
| `--page-width` | 1152px | every centred page surface, via `PageSurface` |
| `--converse-width` | 900px | Chat, Plan, Research transcripts |
| `--read-width` | 46rem | long-form prose: KB docs, artifact bodies, shares |

There was briefly a fourth, `--focus-width`, for the Home inbox column. It was
drift wearing a design's clothes: the inbox is a Home TAB, so holding it at
800px slid the page 352px every time you moved between it and a sibling tab.
**A narrower measure has to be earned by the content, not by the surface's
mood** — the brief's one prose block carries its own `62ch` and wanted nothing
from the frame.

- **`PageSurface` has no `width` prop, deliberately.** An escape hatch would be
  taken within a week by whichever table felt cramped, and one view opting out is
  the drift coming back. A surface that needs more room scrolls its wide child
  (`overflow-x-auto`) instead of widening the page under everything else.
- **A skeleton must be the width of the content that replaces it.** Otherwise the
  skeleton→content swap is itself a width shift, which is the one jump every
  cold load pays.
- **Frames are what must not move; measures may differ.** The three narrow
  tokens exist because their content is read rather than scanned — forcing prose
  to 1152px is ~145 characters a line. Inside a page surface, a field or a prose
  column is sized by what it holds (`max-w-md` on a lone input + button row);
  that is not drift, because the panel edges still line up view to view.

## Pick-one-and-edit-it — use `LibraryPane`

A picker on the left, the record you picked on the right. This had FIVE
independent implementations — Templates, the agent role library, Studio, the
Teams dialog, and the rail views — and they had drifted into different
*behaviour*, not merely different markup: whether the list scrolls with the page
or inside itself, whether a failed read renders as an empty list, whether the
picker sits in a `Panel` or a bare grid, what "nothing selected" says.

`components/ui/LibraryPane.svelte` is the one answer. It owns the two-pane
frame, independent scroll on both sides, the grouped picker with its section
headers, row-shaped loading skeletons, the failure notice, and the
nothing-selected pane. Callers bring data and snippets.

- **Selection is the caller's.** Templates keeps it in the URL, the Teams
  dialog in a local `$state`. A component that insisted on one would have kept
  the other on a private copy of this.
- **Feed it `listQuery`.** It takes `pending` and `notice`, not a raw query, so
  "the read failed" arrives already shaped and cannot be dropped on the way in.
- **Creation belongs to the pane.** Pass `onCreate` (plus `createLabel`) and
  the pane renders the one shared control — `InlineCreate` in `icon` mode, a
  bare left-aligned `+` in the picker footer that expands into an input. Do not
  hand-roll a create field: there were three of them across three views (a raw
  Input+Button under the list, an `InlineCreate`, and a `+` in the header
  opening an empty form), in three positions. The caller receives a trimmed,
  non-empty name and decides what a record made of it is — a saved row, or an
  unsaved draft in the editor. That difference is the caller's; the control
  does not change shape.
- **`bare` inside a dialog.** The Modal is already a surface; a Panel within a
  Panel is a border inside a border with the padding twice. For the same reason
  a detail component rendered into the pane must not wrap itself in a `Panel`.

### Which archetype, when

The distinction is what the right-hand side IS, not how the left-hand side looks:

| | Left | Right | Use |
|---|---|---|---|
| **Rail surface** | context switcher — *which* thing you are working in | a whole workspace | `<RailSurface>` + `<Rail>` |
| **Library pane** | a library of records | a form that edits one | `<LibraryPane>` |

Comms, Knowledge and Artifacts are the first. Templates, the agent role library
and Teams are the second. **Studio is the first and does not use it yet** — it
picks *who you are building for* beside a full authoring workspace, so it wants
`Rail`, not `LibraryPane`; forcing it into a library pane would frame a
page-scroll workspace in a fixed-height panel and break its stagger contract.

## Controls — fewer, smaller, closer

- **Do not label what placement already says.** A button in the footer of the
  Roles library does not read "New role", and one on the Tickets tab of
  Templates does not read "New ticket template" — the reader is standing in the
  thing. It is a `+`. The house form is `<IconButton title="…">`, or
  `<InlineCreate icon>` where the create expands into a field.

  The exception is a page's one or two genuinely primary actions, and anything
  destructive or deploying, which stay labeled.

  **This is about VISIBLE text only.** The tooltip and accessible name go the
  other way and should be specific — "New role", not "New" — because a screen
  reader gets none of the context that placement gives everyone else. `title` on
  `IconButton` is required for exactly this reason, and it sets `aria-label`
  too. Terse on screen, precise in the accessibility tree.

- **Icon buttons with tooltips** (`<IconButton title="…">`) for every action
  whose verb is implied by placement: rail-header "new", surface-header
  settings, row-hover actions. A labeled `<Button>` is reserved for a page's
  one or two real actions, and for anything destructive or deploying.
- **No giant buttons.** `size="sm"` is the default posture for anything in a
  header or rail. Full-width primaries in sidebars are banned.
- **Autosave by default.** Selects, toggles, and pickers apply on change.
  Explicit Save/Apply survives ONLY where the action deploys, restarts
  containers, spends real money, or commits a secret (key entry).
- **One chat width.** Every conversation surface (agent DMs, channels,
  research reports and composers) centers on `--chat-content-max-width`
  (900px). No per-surface widths. The ONE exception: plan mode, where chat sits side by side with the living document and fills its pane.
- **There is no send button.** Enter IS send, in every composer; Esc stops a
  streaming reply. The KeyHint chip beside the input is the affordance.
- **Keyboard-first, visibly.** Composers show contextual KeyHint chips ("⏎
  send", "esc stop") that fade in when the shortcut is live. Motion marks
  interruptible moments: Stop pulses while a reply streams.
- **Attach is a menu, not a file browser.** The paperclip offers Knowledge
  docs, Artifacts, and file upload; knowledge/artifact picks become reference
  chips whose content travels to the model (ACL-checked at attach time).
- **Composer geometry.** Rows are `flex items-end`; h-9 controls carry
  `self-end mb-1`, pill controls `self-end mb-[7px]` — one optical line
  against the resting textarea, bottom-anchored as it grows.
- **No ellipsis in UI copy.** Buttons, placeholders, menu items, and loading
  states use plain words ("Delete channel", "Loading", "Search") — never a
  trailing "…". Content truncation markers on clipped USER text are the only
  tolerated use.
- **Em dashes sparingly.** In UI copy an em dash must earn its place; the
  default is a period, comma, or colon. This applies to agent prose too (the
  rendered soul's voice contract enforces it fleet-wide).
- **Agents converse like colleagues.** The rendered soul carries a voice
  contract: acknowledge (or ask ONE clarifying question) before diving into
  tools, keep process narration out of chat, and report outcomes in a few
  short sentences. Copy surfaces should assume replies are short.
- **Destructive actions are never buttons.** They render as quiet red LINKS
  (`<DangerLink>`): small text, muted until hover, tucked at the edge of the
  section they affect — they must not compete for attention. The in-app
  `confirm()` dialog remains the deliberate step; the trigger stays quiet.

## Primitive-first (post-audit, 2026-07)

A full modularity audit swept the top surfaces onto the kit; these primitives now exist so nobody
re-rolls them (each replaced 5-20 hand-rolled copies):

- **`<Tabs>`** — the underline tab strip; absorbs the `?tab=` deep-link pattern's render half.
- **`<Checkbox>` / `<Radio>` / `<Toggle>`** — accent-styled selection controls, label included.
  A raw `<input type="checkbox">` (native blue in a bronze app) is a bug.
- **`<SectionHeader title info action>`** — the panel-header cluster (title + InfoTip + trailing
  control).
- **`<Segmented>`** — small exclusive mode switches (read/edit). Tabs for page sections; Segmented
  for modes inside a surface.
- **`<DropdownMenu trigger items>`** — anchored menus sharing the context-menu shell/grammar.
  Ad-hoc `absolute top-full` panels with hand-rolled outside-click are banned for action lists
  (search/typeahead popovers remain bespoke).
- **`<Chip>` grew modes** — `tone` (accent/success/warn/danger), filter pills (`onSelect` +
  `selected`), removable tokens (`onRemove`). Filled `rounded-full` pills are a different species;
  don't force them into Chip.
- **`<EmptyState variant="compact" | "inline">`** — panel-scale and one-line zero states; the
  hand-rolled "no X yet" div stays banned at every size.
- **`<SaveButton>` / `useSavedFlash()`** — the "Save → Saved ✓" pattern; no more setTimeout state
  machines.
- **`<CopyButton value|path>`** — every copy affordance flashes the check.
- **`Button` variants** — `danger-outline` (quiet destructive in dense panels), `accent-soft`
  (connect/enable nudges), `link` (inline text actions).

Parity rule for adoption: swapping onto a primitive must not redesign the surface. When a primitive
can't express the existing look without fighting it, keep the bespoke markup and leave a comment —
that's a signal the kit needs a variant, not more overrides.

## Rows, chips, dots

- Selectable list rows: `<RailRow>` (rounded-lg px-2 py-1.5 text-sm,
  hover:bg-card, active bg-card text-fg). Don't fork it.
- Count badges: `<CountPill>`. Kind/status chips: `<Chip>` (bordered,
  `text-[10px] uppercase`). Health/status dots: `<StatusDot>` (h-2 w-2;
  status→color mapping inside).
- Micro-labels (section headers, chips): `text-[10px] font-semibold uppercase
  tracking-wide text-muted` — one size, not 9/10/11px per screen.

## Icon language

Two deliberate registers — don't mix them:
- **Glyphs** (`◈ ⊞ ◎ ❖ ◆ …`) are the BRAND register: nav rail, EmptyState
  icons, welcome moments. (Nav glyphs were deliberately chosen; keep them.)
- **lucide-react** is the ACTION register: toolbars, menus, row actions,
  anything clickable inside content.

## Popovers & panels

- Dropdown menus: one shell — `rounded-xl border border-line bg-card p-1
  shadow-lg z-30`.
- `<Panel>` for cards; the light inset sub-card is `rounded-xl border
  border-line-subtle p-3` (pick 3; stop drifting to 2/4/5).
- `<EmptyState>` for every zero state. The inline "no X yet" div is banned.

## Zero states own their container

`EmptyState variant="full"` is the rendering for a pane that resolved EMPTY, and
it fills that pane edge to edge — the dithered vignette is part of it, and a
vignette that stops short of the container's edge reads as a textured card
floating inside an untextured one.

Two things follow, and both have bitten:

- **Do not wrap it in padding.** The component carries its own inner padding, so
  a `<Panel>` (which is `p-6` by default) or a padded `<div>` around it is a
  band of container the treatment cannot reach. Use `<Panel class="p-0">`.
- **`h-full` is not enough on its own.** It is `height: 100%`, which resolves
  against the parent only if the parent has a DEFINITE height; otherwise it
  computes to `auto` and the zero state collapses to the height of its own three
  lines. `full` carries a `min-h-48` floor for exactly that case, so it reads as
  a region it owns wherever it lands. If a container does have a height, it
  still fills it.

`compact` and `inline` are the other answers: `compact` for a zero state inside
a list or panel that is not the whole surface, `inline` for a single quiet line.
Neither draws a vignette, so neither has this constraint.

## Layers — a surface must differ from what it sits on

Mercury's fills are a hierarchy, and they are what tell a reader that one
region is *inside* another: ground (`bg-surface`) → panel (`bg-panel`) → raised
(`bg-raised`) → hover.

**`Modal` is panel-filled, and so is `Panel`.** A `Panel` inside a `Modal`
therefore paints exactly its container's colour, and the whole region collapses
to a bare outline with nothing behind it. The same is true of a `Panel` inside a
page section that is itself panel-filled.

Inside a panel or a modal, a content region is an **inset well**: `rounded-lg
border border-line bg-surface`. That is already the house form —
`InternalEditorModal`'s preview, the chat composer, `AgentConfigForm`'s soul
box. `LibraryPane` takes `surface="well"` for exactly this, and the components
that wrap it (`SkillsLibrary`, `MemoryPanel`) forward it.

On a page, an unfilled bordered box is fine: the ground behind it already
differs from the border, and it reads as a grouping rather than a surface. The
rule is only about a fill matching its container.

## Color

- Tokens only (`var(--theme-*)`); no hex fallbacks in call sites — the theme
  defines the values once.
- Prefer the class spelling `text-[color:var(--theme-danger)]` over inline
  `style` when static.

## Typography — two voices

- **Mono (`--font-mono`, the default via body)** is the CHROME voice: nav,
  buttons, tabs, tiny uppercase labels, chips, stats, timestamps, and every
  IDENTIFIER (ticket refs, model ids, slugs, keys, config, code).
- **Sans (`--font-sans`, IBM Plex Sans — apply via `font-sans`)** is the
  READING/BUSINESS voice: anywhere someone reads paragraphs or types real
  content. Already wired: all rendered markdown (`<Markdown>`), all TipTap
  content (`.tiptap`), Input/Textarea/Select/Combobox, board card + list
  titles/descriptions, home queue/mail/agenda rows, notification titles,
  KB/artifact content titles.
- Inside a sans surface, identifiers re-assert mono with
  `font-[var(--font-mono)]` (ticket refs on cards do this); `cn()` resolves
  the conflict in the caller's favor on shared controls.
- New content-bearing text → `font-sans`. New chrome → nothing (mono is the
  default). When unsure: would a user READ or TYPE a sentence here? Sans.

## Context menus

- Every row, card, and tile a user can act on gets a right-click menu:
  `useContextMenu()` from `ui/context-menu.tsx`, items at the cursor,
  keyboard-navigable, same shell as dropdowns.
- A context menu is a SHORTCUT: items mirror actions the surface already
  offers (same functions, same confirms, same role guards) — never the only
  home of an action, never a new capability.
- Order: primary action first, Copy link second (`copyAppLink` + the deep
  link), destructive items last behind a `'sep'`, styled danger.
- Native context menus are suppressed app-wide in `_app.tsx`; editable
  fields (input/textarea/contenteditable) keep the browser menu — paste and
  spellcheck are real workflows.

## Deep links — the URL is the selection

- Any selection a user could want to share, revisit, or step back through
  lives in the URL, not in component state: route paths for entities
  (`/boards/:id/:taskId`), search params for view state (`/?tab=boards`,
  `/knowledge?space=…&doc=…`, `/artifacts?a=…`, `/research?r=…`,
  `/comms?c=…` / `?a=…&x=…`, `/plan?p=…`, `/admin?tab=…`, `/settings?tab=…`).
- Pattern: derive the selection FROM `Route.useSearch()` and shadow the old
  setter as a navigate wrapper — call sites don't change. User picks are
  PUSH navigations (back/forward walks them); defaults and healing are
  REPLACE (housekeeping never pollutes history).
- Never the one-shot apply-then-clear pattern — it breaks copy-link.
- New surface or new selection state → wire the param first, then build.

## Loading

- Never render a blank pane or a "Loading" string while a query is in flight —
  use `Skeleton` / `SkeletonRows` / `SkeletonCard`
  (`ui/src/components/ui/Skeleton.svelte`), shaped like the content they stand
  in for so the swap doesn't jump.
- The material is SIGNAL STATIC: a dithered dot field on the house Bayer grid,
  noise re-rolled at 8Hz around a steady mean — an instrument that has not
  acquired its signal yet. `lib/skeleton-static.ts` owns it; a skeleton is a
  transparent box that gets masked out of one page-wide field, so neighbouring
  blocks are windows onto the same material and NOT independent effects.
- Static has no direction and no phase, so there is no `delay` and no stagger
  on a skeleton — sweeps and fills imply a completion a fetch cannot promise.
  Size and shape are the only things a call site chooses (`h-*`, `w-*`, and a
  border radius, where `rounded-full` reads as a capsule or a circle).
- A skeleton stands in for content whose SHAPE is unknown until it arrives.
  Row rails — the leading status dot every row has, in the same place at the
  same size whatever the data turns out to be — are not that: rendering one as
  static claims an uncertainty that does not exist. Give those a flat
  `bg-line` element instead. Not `StatusDot` either, which would imply a status
  nothing yet knows.
- Below that, the material has a FLOOR at roughly 16px. A skeleton is a
  statistical field — around half its cells light — so a box covering only a
  handful of cells lights a handful of dots, and sometimes none. Below the
  floor the field eases toward solid rather than thinning out, so it stays
  visible and stops flickering. The floor is counted in CELLS and so moves with
  the grid pitch; it is rescaled alongside it to hold the same physical size.
  Above the floor, judge by whether the shape is genuinely unknown.
- `Generating` is for MODEL output being written; `Skeleton` is for FETCHES.
  Adjacent languages, different meaning — don't mix them.
- Empty states (`EmptyState`) only render once the query has RESOLVED empty;
  loading must never flash "No X yet".

### Three languages, three questions

| Question | Family | Material |
|---|---|---|
| Has the FETCH resolved? | `Skeleton` / `SkeletonRows` / `SkeletonCard` | signal static |
| What SHAPE is the output? | `Generating` / `GeneratingOverlay` | bar rows sized like the coming text |
| Is the agent still WORKING? | `Waiting` / `WaitingMark` | one of thirty dot-field marks |

- The activity MARK is `WaitingMark`, never a hand-rolled spinner and no longer
  `GeneratingDots`. A call site names a SITE (`site="chat/first-token"`) and
  nothing else: which of the thirty states it draws, and how fast, are decided
  by the rotation in `ui/src/lib/waiting/`.
- Every site is a row in `lib/waiting/sites.ts` with a `role` and a `slot`.
  **Role** is what the wait means (`submitting` / `reasoning` / `tool` /
  `background`) and sets the tempo against spec §9's rungs. **Slot** is where it
  physically sits (`button` / `inline` / `status`) and is what keeps a 5×5 grid
  out of a 28px button. A `site` with no row is a type error — that is
  deliberate, because an undecided mark is an undecided meaning.
- The set is dealt, not hashed. One seed per browsing session (sessionStorage)
  shuffles the catalogue and deals it across every site, so a session shows
  ~30 distinct marks and the same spot keeps the same mark for as long as you
  are looking at it. Adding a site does not re-deal the ones before it.
- Review overrides: `?waiting=<slug>` pins every site to one state,
  `?waiting-seed=<n>` replays a specific hand. Both are read once at boot.
- Use `<Waiting site label>` rather than `<WaitingMark>` unless the surrounding
  row already says what is happening in words. The mark is `aria-hidden` and the
  LABEL is the live region — a screen reader must never be handed a run of
  braille codepoints.
- Marks never blank. Every state holds a floor, because an indicator that empties
  even for two frames reads as *finished*.

## Editors

One rule decides which input a field gets:
- **Prose someone reads later** (descriptions, doc/skeleton bodies, souls,
  personalities, comments) → `RichEditor`. Markdown under the hood
  (`onSave` fires on blur, or debounced with `autosave`); reseed by
  remounting with a `key`, never by mutating `value`.
- **Machine or prompt text** (YAML/config, raw HTML source, AI instructions,
  agent guidance) → `Textarea`, usually mono. Rich formatting in a prompt is
  noise the model has to see past.
- Chat/channel composers stay `Textarea` deliberately: Enter-to-send +
  caret-based @mention autocomplete beat a toolbar there. Rendering is
  always `<Markdown>` on the way out — both directions of a conversation.
- Long-form editing gets an escape hatch: fullscreen toggle (artifacts, KB,
  plan doc) or the slide-in editor (tickets); `InternalEditorModal` when
  version history + muse drafting belong next to the text.
