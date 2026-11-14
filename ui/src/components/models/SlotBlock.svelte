<script lang="ts">
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import ModelIdPicker from '@/components/fleet/ModelIdPicker.svelte'
  import EffortSlotPicker from './EffortSlotPicker.svelte'
  import { CAPABILITY_WORDS, assignmentNotice, suppliedSearchNote, tagTitle, type ModelRow } from './fitness'
  import { useModelCapabilities } from './fitness-queries'
  import type { RoleIssue, Slot } from './slot'

  // ONE SETTING'S ROW, inside its category's detail (CategoryDetail owns the
  // frame and the scroll). A slot is either a model ROLE or a platform WORKER
  // (slot.ts); the row is shared because the record is the same either way,
  // and the few per-kind differences narrow right where they render.
  //
  // EVERY PICK SAVES AT ONCE. An assignment is two controls whose every change
  // is a complete, self-contained decision, so there is no dirty state and no
  // Save: the record's "unsaved" concept does not exist here.
  let {
    slot,
    assigned,
    models,
    effort,
    issues,
    onModel,
    onEffort,
  }: {
    /** The setting this row edits, either kind. */
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

  // Capability facts and the last fitness run's per-slot bands: a need is a
  // word on a quiet line until the assigned model is measured not to have it,
  // which is the exception that earns a warning chip, and
  // the notice is the unfit warning (a run that cannot reach what the role
  // needs, or a run that tested the slot badly). Both advisory; neither
  // blocks. A worker's slot key is the same `agent:<id>` spelling the fitness
  // bands are filed under.
  const capsQuery = useModelCapabilities()
  const capsRow: ModelRow | undefined = $derived(assigned ? capsQuery.data?.models.find((m) => m.id === assigned) : undefined)
  const notice = $derived.by(() => {
    if (!assigned) return null
    const slotKey = slot.kind === 'role' ? `role:${slot.row.role}` : `agent:${slot.row.id}`
    const note = slot.kind === 'role' ? (issues.find((i) => i.role === slot.row.role)?.note ?? null) : null
    return assignmentNotice({ entry: capsQuery.data?.index[assigned], slotKey, capabilityNote: note })
  })
  // A research role whose model cannot browse natively, with a harness tool
  // that can fetch. Not a warning: it says the run will still have pages.
  const supplied = $derived(
    !notice && slot.kind === 'role' && slot.row.requires.includes('search') ? suppliedSearchNote(capsRow) : null,
  )

  // The category already names the family, so "Research · Recon" reads "Recon".
  const name = $derived(slot.row.label.split(' · ').pop() ?? slot.row.label)
  const fixed = $derived(slot.kind === 'agent' && !slot.row.assignable)
  // The catalogs spell skills and fallbacks as lowercase fragments.
  const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)

  // "Search" reads "search" in a sentence, and "JSON" stays as it is.
  const inSentence = (text: string) => (/^[A-Z][a-z]/.test(text) ? text.charAt(0).toLowerCase() + text.slice(1) : text)

  // What a role needs, each read against the assigned model's measured facts.
  const needs = $derived(
    slot.kind === 'role'
      ? slot.row.requires.map((cap) => {
          const words = CAPABILITY_WORDS[cap as keyof typeof CAPABILITY_WORDS]
          const view = capsRow?.capabilities.find((c) => c.cap === cap)
          return {
            cap,
            lacking: view?.state === 'no',
            word: inSentence(words?.short ?? cap),
            lacks: words?.lacks ?? cap,
            title: view ? tagTitle(view) : `This job needs a model that can ${words?.plain ?? cap}.`,
          }
        })
      : [],
  )
  const lacking = $derived(needs.filter((n) => n.lacking))
</script>

<SettingsRow label={name}>
  {#snippet meta()}
    {#if slot.kind === 'role' && !slot.row.wired}
      <Chip title="This takes effect when its feature ships.">Reserved</Chip>
    {/if}
  {/snippet}
  {#snippet hint()}
    <p>{slot.kind === 'role' ? slot.row.hint : slot.row.job}</p>
    {#if needs.length > 0}
      <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span class="font-sans text-xs leading-5 text-muted">
          Needs {#each needs as n, i (n.cap)}{i > 0 ? ', ' : ''}<span title={n.title}>{n.word}</span>{/each}
        </span>
        {#each lacking as n (n.cap)}
          <Chip tone="warn" title={n.title}>{n.lacks}</Chip>
        {/each}
      </div>
    {:else if slot.kind === 'agent' && slot.row.skills.length > 0}
      <!-- A worker declares what its harness already brings, which is why most
           of them want so little from a model. -->
      <p class="font-sans text-xs leading-5 text-muted">Brings {slot.row.skills.join(', ')}</p>
    {/if}
    {#if notice}
      <p class="mt-1.5 max-w-[72ch] text-warning" role="status">{notice.text}</p>
    {:else if supplied}
      <p class="mt-1.5 max-w-[72ch]">{supplied}</p>
    {/if}
  {/snippet}
  {#if fixed && slot.kind === 'agent'}
    <!-- Fixed by design (the person's own assistant): identity, not a slot, so
         there is no picker, and the catalog's own line says what runs it. -->
    <span class="w-[260px] font-sans text-[13px] leading-5 text-muted">{sentence(slot.row.auto)}</span>
  {:else}
    <!-- One column, so every row's model sits in the same place whether or
         not its model takes an effort level. -->
    <div class="flex w-[260px] flex-col gap-2">
      <ModelIdPicker
        {models}
        value={assigned ?? null}
        onChange={onModel}
        emptyLabel="Auto"
        emptySub={slot.kind === 'agent' ? sentence(slot.row.auto) : 'Its fallback chain'}
        class="w-full"
      />
      <EffortSlotPicker model={assigned} value={effort ?? ''} onChange={onEffort} class="w-full" />
    </div>
  {/if}
</SettingsRow>
