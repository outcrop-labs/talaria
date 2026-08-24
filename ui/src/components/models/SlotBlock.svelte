<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import ModelIdPicker from '@/components/fleet/ModelIdPicker.svelte'
  import EffortSlotPicker from './EffortSlotPicker.svelte'
  import CapabilityTags from './CapabilityTags.svelte'
  import { CAPABILITY_WORDS, assignmentNotice, type ModelRow } from './fitness'
  import { useModelCapabilities } from './fitness-queries'
  import { slotState, type RoleIssue, type Slot } from './slot'

  // ONE SETTING'S BLOCK — the record-editor grammar (Templates' document
  // editor) applied to a single "which model runs this" record, rendered
  // inside its CATEGORY's detail (CategoryDetail), which owns the frame and
  // the scroll. A slot is either a model ROLE or a platform WORKER (slot.ts);
  // the anatomy is shared because the record is the same either way, and the
  // few per-kind differences — which state chip, which needs list, what Auto
  // resolves to — narrow right where they render.
  //
  // THE LAYOUT RULE THE CONTEXT DEMANDS: an editor saves on the record's one
  // Save button because a document is a burst of edits; an assignment is two
  // controls whose every change is a complete, self-contained decision — so
  // each pick PUTs at once (the same semantics the stacked rows had) and there
  // is no dirty state, no Cancel, no Save menu to pin. The record's "unsaved"
  // concept does not exist here, and inheriting it would have made a
  // two-control surface pay for a document editor's machinery.
  let {
    slot,
    assigned,
    models,
    effort,
    issues,
    onModel,
    onEffort,
  }: {
    /** The setting this block edits, either kind. */
    slot: Slot
    /** The assigned model id, or undefined for Auto. */
    assigned: string | undefined
    /** Assignable gateway model ids. */
    models: string[]
    /** The slot's stored effort preference (null = the model's own default). */
    effort: string | null
    /** The server's role-capability issues (advisory), for the assignment notice. */
    issues: RoleIssue[]
    onModel: (model: string | null) => void
    onEffort: (effort: string | null) => void
  } = $props()

  // Capability facts and the last fitness run's per-slot bands: the tags are
  // what this model has MEASURED to be, the notice is the unfit warning (a
  // capability recorded false, or a run that tested the slot badly). Both
  // advisory; neither blocks. A worker's slot key is the same `agent:<id>`
  // spelling the fitness bands are filed under.
  const capsQuery = useModelCapabilities()
  const capsRow: ModelRow | undefined = $derived(assigned ? capsQuery.data?.models.find((m) => m.id === assigned) : undefined)
  const notice = $derived.by(() => {
    if (!assigned) return null
    const slotKey = slot.kind === 'role' ? `role:${slot.row.role}` : `agent:${slot.row.id}`
    const note = slot.kind === 'role' ? (issues.find((i) => i.role === slot.row.role)?.note ?? null) : null
    return assignmentNotice({ entry: capsQuery.data?.index[assigned], slotKey, capabilityNote: note })
  })
</script>

<!-- The setting's title row: kind chip + name + state, the record-editor
     anatomy. -->
<div class="mb-2 flex items-center gap-2">
  <Chip class="shrink-0">{slot.kind === 'role' ? 'role' : 'worker'}</Chip>
  <div class="min-w-0 truncate text-sm font-semibold text-fg">{slot.row.label}</div>
  {#if slot.kind === 'role' && !slot.row.wired}
    <Chip class="shrink-0" tone="neutral" title="This slot takes effect when its surface lands.">reserved</Chip>
  {:else if slot.kind === 'agent' && !slot.row.assignable}
    <Chip class="shrink-0" tone="neutral" title="Its persona and privacy are the point — not a slot.">fixed</Chip>
  {/if}
  <span class="ml-auto shrink-0 font-mono text-[11px] text-muted">{slotState(slot, assigned)}</span>
</div>
<p class="mb-3 text-xs text-muted">{slot.kind === 'role' ? slot.row.hint : slot.row.job}</p>

{#if slot.kind === 'agent' && !slot.row.assignable}
  <!-- Fixed by design (the user's own assistant): identity, not a slot —
       there is no picker to render, and the catalog's own line about it is
       the whole explanation. Same card frame a pickable assignment gets so
       the record keeps its fields-card anatomy. -->
  <div class="rounded-lg border border-line bg-card/40 p-3">
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model</div>
    <p class="font-sans text-xs text-muted">{slot.row.auto}</p>
  </div>
{:else}
  <!-- THE FIELDS CARD: the assignment. Effort and model share a cluster
       because the dial belongs to the model semantically — it is the model's
       own ladder — exactly as the stacked row grouped them. -->
  <div class="mb-3 shrink-0 space-y-3 rounded-lg border border-line bg-card/40 p-3">
    <div>
      <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Model</div>
      <div class="flex items-center gap-1.5 rounded-lg border border-line bg-surface p-1">
        <EffortSlotPicker model={assigned} value={effort ?? ''} onChange={(v) => onEffort(v || null)} />
        {#if assigned}
          <span class="h-5 w-px shrink-0 bg-line" aria-hidden="true"></span>
        {/if}
        <ModelIdPicker {models} value={assigned ?? null} onChange={onModel} emptyLabel="Auto" bare class="min-w-0 flex-1" />
      </div>
    </div>
  </div>
{/if}

{#if notice}
  <p class="mb-3 max-w-prose font-sans text-xs text-warning">{notice.text}</p>
{/if}

<!-- The requirement/pitch half of the advisory pairing. A ROLE declares
     what its work needs from a model (empty is a claim, not a shrug —
     utility genuinely runs on anything); a WORKER declares what its harness
     already brings, which is why most of them want so little. -->
{#if slot.kind === 'role'}
  <div class="mb-3">
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">This role needs</div>
    {#if slot.row.requires.length > 0}
      <span class="inline-flex flex-wrap items-center gap-1">
        {#each slot.row.requires as c (c)}
          <Chip>{CAPABILITY_WORDS[c as keyof typeof CAPABILITY_WORDS]?.short ?? c}</Chip>
        {/each}
      </span>
    {:else}
      <span class="font-sans text-xs text-muted">Nothing specific — a fast, cheap model is ideal.</span>
    {/if}
  </div>
{:else if slot.row.skills.length > 0}
  <div class="mb-3">
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Brings</div>
    <span class="inline-flex flex-wrap items-center gap-1">
      {#each slot.row.skills as s (s)}
        <Chip>{s}</Chip>
      {/each}
    </span>
  </div>
{/if}

{#if assigned}
  <!-- The other half: what the assigned model has MEASURED to be — the full
       record, not the negative-only cut a dense stacked row showed, because
       here there is room for the whole truth. -->
  <div>
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Measured capability</div>
    <CapabilityTags row={capsRow} />
  </div>
{:else if slot.kind === 'agent' && slot.row.assignable}
  <!-- A worker's Auto is structured data — the pane can quote exactly what
       the chain resolves to instead of the generic line a role gets. (A
       fixed worker shows nothing here; its card above said it all.) -->
  <p class="max-w-prose font-sans text-xs text-muted">Auto: {slot.row.auto}.</p>
{:else if slot.kind === 'role'}
  <p class="max-w-prose font-sans text-xs text-muted">
    Auto leaves this role on its documented fallback chain — assign a model to pin it.
  </p>
{/if}
