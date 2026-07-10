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
- **Send is an icon** (ArrowUp, `title="Send — Enter"`); Enter submits in
  every composer. No "Go", no wide "Send".
- **No ellipsis in UI copy.** Buttons, placeholders, menu items, and loading
  states use plain words ("Delete channel", "Loading", "Search") — never a
  trailing "…". Content truncation markers on clipped USER text are the only
  tolerated use.
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
