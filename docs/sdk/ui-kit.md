# UI kit

The same Mercury primitives the platform is built from — visual parity with core surfaces is
automatic, and nothing needs installing. Full export list: [reference.md](./reference.md).

```svelte
<script lang="ts">
  import { Button, Chip, Input, fade, QUICK } from '@talaria/sdk'

  let name = $state('')
</script>

<div class="flex items-center gap-2" in:fade={QUICK}>
  <Input bind:value={name} placeholder="Name" size="sm" />
  <Button size="sm" disabled={!name.trim()}>Save</Button>
  <Chip>draft</Chip>
</div>
```

Icons come from `@lucide/svelte` (host-resolved — note the unscoped `lucide-svelte` is an
unrelated package).

## Picking a component

| you need | use |
| :--- | :--- |
| A click action | `Button` (variants: primary, outline, ghost, danger, danger-outline, accent-soft, link) |
| An icon-only action | `IconButton` (sizes include the bordered tile) |
| Text in / text out | `Input`, `Textarea`, `Select`; searchable pick-list → `Combobox` |
| A toggle | `Toggle`; one-of-many → `Radio`; yes/no → `Checkbox` (bare cell form for tables) |
| A card | `Panel` (+ `SectionHeader` inside); a metric tile → `StatCard` |
| A dialog | `Modal`; an anchored panel → `Popover`; an anchored menu → `DropdownMenu` |
| Confirmation | `confirm()` / `alert()` / `prompt()` — the platform's dialogs, not the browser's |
| Tabs / a switch | `Tabs`, `Segmented` |
| A label pill | `Chip` (tones; filter pills via onSelect/selected; removable tokens via onRemove) |
| Empty / loading / working | `EmptyState` · `Skeleton`/`SkeletonRows`/`SkeletonCard` · `Generating*`/`Waiting*` |
| A read's three answers | `QueryState` (loading, broken, resolved) · a list → `listQuery` · the error alone → `QueryError` |
| Rich text | `RichEditor` (WYSIWYG, markdown stored); render markdown → `Markdown` |
| Add a record by name | `AddRow` (always open, names its action); the click-to-reveal `+` → `InlineCreate` |
| Rename in place | `RenameField`; a record's emoji + title heading → `RecordTitle` |
| Key handling | `submitOnEnter` (Enter presses the button) · `inlineEditKeys` (Enter commits via blur, Escape reverts) |
| A sortable column head | `SortHeader` (the affordance; the sort cycle stays yours) |
| A library beside its detail | `LibraryPane` (picker on the left, the selected record on the right) |

The three loading states are deliberately distinct: **Skeleton** = a fetch hasn't resolved,
**Generating** = model output is being written, **Waiting** = an agent is working right now.
An app declares its own waiting site inline, and the `label` is required — it is the live
region a screen reader announces (the mark itself is decorative):

```svelte
<script lang="ts">
  import { Waiting } from '@talaria/sdk'
</script>

<Waiting site={{ key: 'my-app/summarise', role: 'reasoning' }} label="Summarising the thread" />
```

A failed read is a fourth state, and never an empty one: render it through `QueryState` or
`listQuery` ([client.md](./client.md#rendering-a-read-loading-broken-or-empty)), so a 500 reads
as "Could not load …" with a Retry, not as "No items yet".

## Motion

Motion comes through the SDK, never `svelte/transition` directly: the wrappers degrade to a
quick fade when the OS asks for reduced motion, and the presets (`QUICK`, `POP`, `PANEL`,
`LIST`) carry the platform's own durations. Usage is identical —
`<div transition:fade>`, `<div in:fly={{ y: 8 }}>`. The full story:
[`ui/ANIMATIONS.md`](../../ui/ANIMATIONS.md).

## Conventions

- Styling classes combine with `cn`; design tokens are Mercury's (`--theme-*` variables —
  see [`design/mercury-spec.md`](../design/mercury-spec.md)).
- When to use what, with screenshots of each host usage:
  [`UI-CONVENTIONS.md`](../UI-CONVENTIONS.md).
- Destructive actions are safety-orange and say what they do — `confirm()` before damage.
