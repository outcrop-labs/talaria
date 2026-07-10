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
2. **Page surface** (Inbox, Agents, Models, Cost, …): `h-full overflow-y-auto
   p-8` → `mx-auto max-w-5xl space-y-6` (Inbox may use 6xl for its two-zone
   layout). Title row: `<h1 className="mercury-text text-2xl font-semibold">`.

Components: `components/app/surface.tsx` (`RailSurface`, `Rail`, `Stage`,
`StageHeader`, `RailSection`, `RailRow`, `CountPill`).

## Controls — fewer, smaller, closer

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

## Color

- Tokens only (`var(--theme-*)`); no hex fallbacks in call sites — the theme
  defines the values once.
- Prefer the class spelling `text-[color:var(--theme-danger)]` over inline
  `style` when static.
