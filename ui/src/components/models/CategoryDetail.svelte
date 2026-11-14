<script lang="ts">
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import SlotBlock from './SlotBlock.svelte'
  import type { RoleIssue, Slot } from './slot'

  // The selected job's half of the Roles pane: its name, one sentence on what
  // it is for, then one row per stage or setting, divided rather than nested.
  // The pane owns the frame; this owns the scroll.
  let {
    label,
    blurb,
    info,
    slots,
    models,
    issues,
    assignmentOf,
    effortOf,
    onSave,
  }: {
    label: string
    blurb: string
    /** The longer explanation, behind an InfoTip beside the name. */
    info?: string
    slots: Slot[]
    /** Assignable gateway model ids (either catalog serves the same list). */
    models: string[]
    /** The server's role-capability issues (advisory), passed through to rows. */
    issues: RoleIssue[]
    assignmentOf: (slot: Slot) => string | undefined
    effortOf: (slot: Slot) => string | null
    onSave: (slot: Slot, patch: { model?: string | null; effort?: string | null }) => void
  } = $props()
</script>

<div class="h-full overflow-y-auto">
  <header class="px-5 pb-4 pt-5">
    <div class="flex items-center gap-2">
      <h2 class="font-sans text-[15px] font-semibold leading-5 text-fg">{label}</h2>
      {#if info}<InfoTip text={info} />{/if}
    </div>
    <p class="mt-1 max-w-[72ch] font-sans text-[13px] leading-5 text-muted">{blurb}</p>
  </header>

  <div class="border-t border-line-subtle [&>*+*]:border-t [&>*+*]:border-line-subtle">
    {#each slots as slot (slot.id)}
      <SlotBlock
        {slot}
        assigned={assignmentOf(slot)}
        {models}
        effort={effortOf(slot)}
        {issues}
        onModel={(model) => onSave(slot, { model })}
        onEffort={(effort) => onSave(slot, { effort })}
      />
    {/each}
  </div>
</div>
