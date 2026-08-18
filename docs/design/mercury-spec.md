# Mercury — Talaria UI Design Specification

**This document is the source of truth.** Values below are **exact** unless marked
(approx), and the implementation is expected to match them rather than any external file.

_Provenance: originally extracted 2026-07-31 (45 artboards, via computed styles + JSX
export) from a Paper exploration titled "Gentle dew". That file lives in a personal
account, so it is credited here as the design's origin and is deliberately NOT the
reference — a house design system cannot depend on a third-party document one person
can revoke. Artboard IDs are kept inline only as breadcrumbs for anyone who still has
access._

**Canonical (follow exactly):** color foundations, type, spacing, Icon Rail, Sidebar, LOGO,
Composer/Default, hover states, motion grammar.
**Reference-only (adapt to Talaria's existing screens/IA):** all full-screen explorations
(Comms 01–10, Inbox variants, Home H1–H5, dashboards, Settings board). Use their *patterns*
(panel treatment, section headers, tables, buttons, toggles) — do not invent new screens.

---

## 1. Color foundations (artboard `VZ-0` — "20 · Factory Foundations")

Tagline from the board: *"Neutral-first operational UI. Near-black surfaces, crisp readout,
and safety orange used only for action, failure, and motion peaks."*

### Instrument surfaces (dark — the primary theme)

| Role            | Hex       | Usage |
|-----------------|-----------|-------|
| Ground          | `#090A09` | App background; also *inset* wells (composer prompt field) |
| Panel           | `#141312` | Panels, cards, sidebar/rail surface, composer body |
| Raised          | `#1E1C1A` | Raised elements: active nav tile, chips, inputs, nested cards |
| Hover           | `#24221F` | Hover fill on rows/items/buttons |
| Hairline        | `#302D29` | Default 1px borders / separators / rail dividers |
| Hairline strong | `#4A4640` | Emphasized borders: composer container, primary chips, avatar ring |
| Readout         | `#E7E2DB` | Primary text ("crisp readout" cream) |
| Ink dim         | `#5F5A53` | Dimmest text: section headers (WORK/MANAGE/SYSTEM) |

### Signal spectrum ("meaning, not chrome")

| Role        | Hex       | Usage |
|-------------|-----------|-------|
| Muted       | `#8E877E` | Secondary text, inactive icons/labels, placeholders, counts |
| Success     | `#A0CA92` | Healthy / connected / online dots, positive deltas |
| Warning     | `#C8B46C` | Attention / degraded |
| Danger      | `#EE6018` | Safety orange — failure, destructive, alerts, motion peaks ONLY |
| Chart blue  | `#68B6C8` | Data viz series |
| Chart coral | `#D77968` | Data viz series 2 |

### Accent — warm gold `#C8B46C` (exact)

**The accent and the warning color are the same hex** — one warm gold anchors the brand:
logo mark fill, sidebar active accent bar, composer submit button, chip meter bars, MCP dot,
primary buttons, toggle knobs (on). Use `#C8B46C` with **dark text/glyphs on gold fills**
(ground `#090A09`). Replace Mercury's bronze (`#b98f5a`/`#d0aa76`) everywhere.

### Semantic rules
- Healthy/connected → `#A0CA92` · Attention/degraded → `#C8B46C` · Failure/destructive → `#EE6018`.
- Orange is never decorative. Primary CTAs = gold fill; destructive = **orange outline**
  (orange border + orange mono label on dark, never orange fill) — see `8A-0` DANGER ZONE.

### Derived light theme (Talaria decision — not in the Paper file)
Light mode stays functional, re-derived from the same language: warm paper-white ground
(`#F2F0EB` family), panel `#FAF8F4`, raised `#FFFFFF`, hairline `#DCD6CC` (strong `#C9C2B6`),
text `#1B1917`, muted `#6E675D`, dim ink `#8A8378`, signals darkened for contrast (success
`#5E7F4E`, warning/accent gold `#8F7A33`, danger `#C8500F`, charts `#3E7A8A` / `#B05A4A`).
Dark is the **default** theme.

## 2. Typography (board `VZ-0` "TYPE / SANS + MONO" + measured components)

Fonts already loaded in Talaria: **IBM Plex Sans** + **IBM Plex Mono** (keep; the design's
"System Sans-Serif" maps to IBM Plex Sans as Talaria's sans voice).
Two voices:
- **Sans** — reading/business voice: headings, body, message text, nav labels, input content.
- **Mono** — chrome voice: section headers, chips, metadata, telemetry, breadcrumbs, counts,
  identifiers, button labels. Uppercase + letterspacing.

Scale:

| Step | Size/line | Style (measured) |
|------|-----------|-------|
| Board heading   | 28px | Sans, semibold, tight tracking |
| Section title   | 18px | Sans, semibold |
| Interface copy  | 14/20 | Sans (body, composer placeholder) |
| Nav / row label | 13/16 | Sans; active w500 `#E7E2DB`, inactive w400 `#8E877E` |
| Wordmark        | 13/16 | Sans semibold, tracking **0.18em**, `#E7E2DB` |
| Metadata        | 11px | Mono, muted |
| Telemetry/labels| 10/12 | Mono; tracking 0.05em (counts) – 0.08em (headers/footer); UPPERCASE |

Section header pattern: 10px mono UPPERCASE tracking 0.08em, ink-dim `#5F5A53`, often with
right-aligned mono meta on the same row (e.g. `CHANNELS ……… 08 LIVE`).

## 3. Spacing / radii (measured)

- 4px base grid. Named steps: **4 = tight**, **6 = control**, **8 = panel**.
- Radii: **6px** is the control radius (nav rows, rail tiles, chips, buttons, inputs,
  composer inset, send button); **8px** panels/cards/composer container; ~10–12px modals;
  `999px` dots/avatars.
- Shadows: matte and rare — composer floats with `0 8px 24px rgba(0,0,0,0.28)`. No glows.

## 4. LOGO (artboard `66P-0`, exact from JSX export)

```html
<svg width="20" height="20" viewBox="0 0 20 20">
  <path d="M10 2L18 18H12.6L10 12.4L7.4 18H2L10 2Z" fill="#C8B46C" />
</svg>
<span style="font:600 13px/16px var(--font-sans); letter-spacing:.18em; color:#E7E2DB">TALARIA</span>
```
- Gold "A"-delta mark (notched triangle) + spaced-caps wordmark; 10px gap (`gap-2.5`).
- Sits at the **top of the sidebar**; mark-only in a 36×36 tile at the top of the icon rail.
- Replaces `ui/src/components/brand.tsx` (WingMark + mercury-text gradient). Keep the
  `Brand`/`WingMark` component API; swap visuals to the exact SVG + wordmark above.

## 5. Icon Rail (`69Q-0`) + Sidebar (`66O-0`) — one collapsible component

Talaria decision: the rail is the **collapsed state** (64px), the sidebar the expanded state
(208px). Persist choice (localStorage) + toggle affordance; tooltips on rail hover.

### Sidebar (expanded, 208px — all values measured)
- Container: `w-208px`, bg `#141312`, border-right 1px `#302D29`, padding 12px inline /
  20px top / 16px bottom; column flex.
- Top: LOGO row (mark + wordmark) — 183×24 zone.
- Sections `WORK` / `MANAGE` / `SYSTEM`: header rows 24px tall, 10px mono 0.08em `#5F5A53`.
- Item rows: 30px tall — padding 7px block / 8px inline, radius 6, gap **9px**; layout:
  [3×14 accent bar, radius 2] [16×16 icon] [13px sans label] [spacer] [10px mono count].
  - Active: bg `#1E1C1A`; accent bar `#C8B46C`; label w500 `#E7E2DB`; icon readout-toned.
  - Inactive: transparent bg; accent bar transparent (slot always reserved); label w400
    `#8E877E`; icon muted.
  - Hover: bg `#24221F`, label → `#E7E2DB`, ~120ms ease.
- Counts right-aligned: 10px mono, tracking 0.05em, `#8E877E` (Inbox `8`, Agents `21`).
- Footer pinned bottom (183×20): 6px round dot `#A0CA92` + `LOCAL · OPERATOR` 10px mono
  0.08em `#8E877E`. (Applied boards add a `BUILD x.y.z` second line — optional.)

### Icon Rail (collapsed, 64px — all values measured)
- Container: `w-64px`, bg `#141312`, border-right 1px `#302D29`, padding 12px top / 20px
  bottom, items centered.
- Top: Talaria glyph tile 36×36 (20×20 gold SVG mark centered).
- Nav items: 36×36 tiles, radius 6; active = bg `#1E1C1A` (icon slightly larger ~23px,
  readout-toned); inactive = transparent, 16×16 muted icon; hover = `#24221F` + tooltip.
- Section dividers: 24×1 `#302D29`.
- Bottom (after flex spacer + 12px gap): avatar 26×26 round, bg `#1E1C1A`, border 1px
  `#4A4640`, initials 10px mono w500 tracking 0.05em `#8E877E`.

### Nav mapping (Talaria decision: keep routes, adopt structure)
- `WORK`: Inbox (/), Comms, Plan, Boards, Research, Knowledge, Artifacts (+ enabled app work
  surfaces) — from `ui/src/lib/nav.ts` NAV.
- `MANAGE`: Agents, Models, MCP, Templates, Agent Studio, Observability, Apps (+ app manage
  surfaces). Same grant-gating as today.
- `SYSTEM` (new section): Settings, Admin (role-gated) — moved from the user menu into the
  sidebar per design. User menu keeps profile/theme/sign-out.
- Replace unicode-glyph icons (`▽ ◈ ⊞ …`) with lucide icons (outline, ~1.5px stroke, 16px):
  Inbox→inbox, Comms→message-circle, Plan→calendar-range, Boards→layout-grid,
  Research→folder-search, Knowledge→book-open, Artifacts→file-box, Agents→bot, Models→cpu,
  MCP→plug-zap, Templates→layout-template, Studio→settings-2, Observability→activity,
  Apps→hexagon, Settings→settings, Admin→shield.

## 6. Top strip (from `1W9-0` "Talaria Comms Shell")

Replaces the current global header (logo moves into the sidebar):
- Left: breadcrumb `LOCAL COMMAND SURFACE / <VIEW>` — 10px mono UPPERCASE 0.08em muted.
- Center/right: compact search (`⌕ SEARCH ⌘K`) — raised tile, hairline border, mono 10px.
- Far right: green status dot + account email (11px mono muted) → opens the user menu.
- Height ~48–56px, ground surface, hairline bottom border.

## 7. Composer (`5ZB-0` + chip strip `60H-0`) — all values measured

The signature component. Applies to chat, channels/comms, and any prompt surface.

### Container (700×142 in the artboard; width fluid in-app)
- bg `#141312`, border 1px **`#4A4640`**, radius **8**, padding **8**, column gap **8**,
  shadow `0 8px 24px rgba(0,0,0,0.28)`.

### Prompt field (inset well)
- bg **`#090A09`** (ground inset!), border 1px `#302D29`, radius 6, padding 14; min-height
  ~76px; textarea transparent, 14/20 sans `#E7E2DB`; placeholder `#8E877E`:
  `What would you like <agent> to work on?` (agent name from the selected agent).
- Send button inside, top-right: **36×36, radius 6, bg `#C8B46C`**, dark up-arrow glyph
  (~16px, stroke ground `#090A09`). Disabled: raised tile + muted glyph.

### Control rail (bottom row, 40px zone; chips 36px tall)
Left→right: `+` attach (36×36, border `#302D29`, mono `+`); agent chip (border
**`#4A4640`**, text 10px mono `#E7E2DB`, + meter: five 3×12 bars, gap ~2, lit bars
`#C8B46C`, unlit raised-tone); model chip `✳ FABLE 5` (same anatomy, 10px glyph icon);
mode chip `NORMAL MODE` (border `#302D29`, muted text); `● MCP 3` (7px gold dot + mono);
`SKILLS 11`; then flex spacer; `?` help and mic icon tiles (36×36, border `#302D29`).
- All chips radius 6, paddingInline 10, mono 10/12.
- Primary chips (agent/model) read brighter: border `#4A4640` + readout text; secondary
  chips: border `#302D29` + muted text → readout on hover.
- Keyboard-focused chip (from `3Q-0`): **dashed gold outline** — adopt for focus-visible.
- Map to real Talaria controls (`ui/src/components/chat/*`): agent-picker → agent chip,
  tier-picker → model chip, modes → mode chip, live MCP server count → MCP chip, skills
  count → skills chip. Only show chips whose feature exists — no fabricated data.

### Popovers (model menu pattern, `3Q-0`)
- Panel `#141312`, hairline border, radius ~10px, search field on top (mono placeholder,
  `⌘K` hint), section headers 10px mono `#5F5A53`, rows 13–14px with right-aligned mono
  meta (pricing/multiplier tags), hover `#24221F`, selected row dashed gold outline.

## 8. Shared surface patterns (from applied boards `44Y-0`, `1W9-0`, `8A-0`, `4W9-0`)

- **Panels/cards:** `#141312` on ground, 1px `#302D29`, radius 8; header = mono uppercase
  dim label + right-aligned mono meta/badge; zones separated by hairlines.
- **Stat blocks:** big sans numerals (24–32px readout) + 10px mono uppercase label,
  optional delta in signal color.
- **Tables/lists:** hairline row separators, 13–14px sans primary cell, mono for numbers/
  IDs/timestamps, row hover `#24221F`.
- **Buttons:** primary = gold `#C8B46C` fill, ground-dark mono uppercase label, radius 6
  (`NEW MESSAGE +`); secondary = raised tile + hairline + readout mono label; ghost = mono
  uppercase muted → readout; destructive = **orange text + orange border outline** (never
  fill), e.g. `CLOSE WORKSPACE`.
- **Toggles:** pill ~36×20; on = gold knob/warm track; off = muted knob raised track;
  locked = dimmed (from `8A-0` CONTROL STATES).
- **Inputs/selects:** raised tile bg, hairline border, radius 6, 13–14px sans content,
  focus = gold border + ring; option rows per popover pattern.
- **Status dots:** 6–7px round — green healthy, gold attention, orange failure; paired with
  10px mono uppercase labels (`● LOCAL`, `● SYNCED 14:32`).
- **Danger zone:** orange hairline panel, `DANGER ZONE` orange mono label + `IRREVERSIBLE`
  right meta, muted body, destructive outline button.
- **Avatars:** 26px round (rail) or small squares, raised tile + `#4A4640` ring, mono
  initials.
- **Focus-visible:** gold — solid 2px ring on controls, dashed gold outline on chips.

## 9. Motion grammar (boards `ED-0`, `IZ-0`, `158-0`)

Policy: 01 Purposeful (orient, confirm activity, expose stage) · 02 Perceived speed (visible
progress feels faster than waiting) · 03 Frequency (frequent loaders stay shorter/subtler) ·
04 Spatial continuity (keep the indicator attached to its work surface).

### Timing ladder (loop budgets)
| Tier | Use | Duration |
|------|-----|----------|
| Immediate | Inline tool response | 600–900ms |
| Standard  | Agent stage / fetch  | 1.0–1.4s |
| Ambient   | Background monitor   | 1.8–2.4s |

### State mapping (semantic loaders)
| State | Motif | Character |
|-------|-------|-----------|
| Submitting   | SIGNAL WEAVE (fast)   | quick bar-weave burst, gold/warm bars |
| Reasoning    | CONTEXT HELIX (loop)  | looping multi-color bar helix |
| Tool activity| STAGE SCAN (scan)     | stepped scan across segment bars |
| Background   | MONITOR BREATHE (idle)| slow breathing opacity on status bars |
| Fetching     | SIGNAL STATIC (idle)  | skeletons as a dithered dot field, per-cell noise at 8Hz |

Motif vocabulary: weave, pulse, scan, breathe, spark, helix, bounce, cascade, static →
implemented as stagger, loop, stepped animation, pulse, orbit, dither. Loaders are rows of small
rounded bars (3×12px, like the chip meters) in surface/signal colors.

SIGNAL STATIC is the one motif that is DENSITY rather than opacity: skeletons render on the same
Bayer grid as the dither engine (§ matte rule — no glows, no blurs), so waiting is a texture the
surface already speaks rather than a second effect layered on it. It carries no direction and no
stagger, because a fetch has no progress to report; under reduced motion the clock stops and the
field freezes on one roll of the noise, which keeps the texture rather than flattening it.

### Rules
- Hover/state transitions: ~120ms ease. Entrances: 150–250ms, small translate + fade
  (framer-motion available). No parallax, no glows.
- **Reduced motion:** replace travel/orbit with a two-state opacity pulse; preserve the same
  status label and semantic color (`prefers-reduced-motion`).

## 10. Chat workspace patterns (board `3Q-0`)

- Session list left panel: `+ New session` row, search row with `⌘K`, `SESSIONS` mono header
  with filter icon, project folder group, session rows (status dot, 13px title, 10px mono
  meta `gRPC · active`, right mono relative time). Active row raised + gold dot.
- Thread header: 15–16px sans title + chevron, right `● SYNCED hh:mm` mono.
- Tool call rows: compact mono rows `codebase / search (12)` with chevron, hairline separated.
- Agent status headline ("Done") 16px sans semibold; body 14px; mono result lines muted.
- Proposed-action card: hairline card, gold left bar, `PROPOSED ACTION` 10px mono muted,
  13px sans body, right-aligned outline button (`REVIEW` mono uppercase).
- User/assistant messages flatten onto the panel (no heavy bubbles) — small avatar square +
  name + mono time, body 14px sans.

## 11. Current Talaria implementation map (what gets touched)

- Tokens: `ui/src/styles.css` (405 lines; `--theme-*` under `[data-theme='mercury']` /
  `[data-theme='mercury-light']`, Tailwind `@theme inline` ~line 164, hljs palettes, tiptap
  styles). Token names keep their contract (`--theme-bg`, `--theme-panel`, …). New tokens:
  `--theme-raised` (#1E1C1A), `--theme-hover` (#24221F), `--theme-border-strong` (#4A4640),
  `--theme-ink-dim` (#5F5A53), `--theme-chart-1` (#68B6C8), `--theme-chart-2` (#D77968),
  motion durations (`--motion-immediate/standard/ambient`), and re-point accent trio to
  `#C8B46C`. The `--mercury-grad` brand gradient dies with the old logo.
- Themes: keep `mercury` (dark, default) + `mercury-light` selectors and the `.dark` class
  mechanism in `__root.tsx`; only values change.
- Shell: `ui/src/routes/_app.tsx` (header + NavRail layout → rail/sidebar + top strip),
  `ui/src/components/app/nav-rail.tsx`, `ui/src/lib/nav.ts` (SYSTEM section), user menu in
  `_app.tsx`, `ui/src/components/brand.tsx`, `ui/src/components/mercury-backdrop.tsx`
  (retire or re-tune to ground), `ui/src/components/theme-toggle.tsx`.
- Composer: `ui/src/components/chat/chat-composer.tsx`, `composer-buttons.tsx`,
  `composer-picker.tsx`, `agent-picker.tsx`, `tier-picker.tsx`, `attachments.tsx`.
- Primitives (37 files): `ui/src/components/ui/*` — avatar, button, checkbox, chip,
  close-button, code-block, combobox, confirm, context-menu, copy-link-button, disclosure,
  emoji-picker, empty-state, field-pill, generating (→ motion motifs), icon-button, info-tip,
  inline-create, input, kbd, markdown, mention-suggest, modal, panel, rich-editor,
  save-button, section-header, segmented, select, skeleton, slash-commands, stat-card,
  steps, tabs, textarea (+ control.ts, editor-behavior.ts).
- Screens (all must land): routes under `ui/src/routes/_app/` — index (Inbox landing), inbox,
  chat, comms, channels, plan, boards (index/$boardId/$boardId.$taskId), research, knowledge,
  artifacts, agents, fleet, models, mcp, templates, studio, observability, apps,
  x.$app.index, x.$app.manage, settings, admin — plus `login.tsx`, `join.tsx`, `a.$slug.tsx`,
  `kb.$slug.tsx`, `kb.space.$slug.tsx`, and feature components under `components/{admin,
  artifacts, assistant, auth, board, chat, fleet, kb, observability, workflows}`.
- Embedded app: `apps/contacts/app.tsx` renders inside `x.$app` — restyle if it carries its
  own colors.
- Only ~19 hardcoded hex occurrences in 5 tsx files — sweep them onto tokens.
