<script lang="ts">
  import SlotBlock from './SlotBlock.svelte'
  import type { RoleIssue, Slot } from './slot'

  // The selected category's half of the Roles pane — the record-editor
  // grammar's DETAIL side for a record whose fields are SETTINGS: the
  // category is the menu item, and its members render as one assignment block
  // each, divided rather than nested, so every setting keeps the full
  // record anatomy (state chip, hint, fields card, advisory context) instead
  // of being demoted to a row inside a table. The pane owns the frame; this
  // owns the scroll.
  let {
    label,
    blurb,
    slots,
    models,
    issues,
    assignmentOf,
    effortOf,
    onSave,
  }: {
    label: string
    blurb: string
    slots: Slot[]
    /** Assignable gateway model ids (either catalog serves the same list). */
    models: string[]
    /** The server's role-capability issues (advisory), passed through to blocks. */
    issues: RoleIssue[]
    assignmentOf: (slot: Slot) => string | undefined
    effortOf: (slot: Slot) => string | null
    onSave: (slot: Slot, patch: { model?: string | null; effort?: string | null }) => void
  } = $props()
</script>

<div class="h-full overflow-y-auto px-4 py-4">
  <!-- The title row: the category IS the record; its members are the fields. -->
  <div class="mb-1 flex items-baseline gap-2">
    <div class="text-base font-semibold text-fg">{label}</div>
    <span class="ml-auto shrink-0 font-mono text-[11px] text-muted">{slots.length} {slots.length === 1 ? 'setting' : 'settings'}</span>
  </div>
  <p class="mb-4 max-w-prose text-xs text-muted">{blurb}</p>

  <div class="divide-y divide-line-subtle">
    {#each slots as slot (slot.id)}
      <div class="py-4 first:pt-0 last:pb-0">
        <SlotBlock
          {slot}
          assigned={assignmentOf(slot)}
          {models}
          effort={effortOf(slot)}
          {issues}
          onModel={(model) => onSave(slot, { model })}
          onEffort={(effort) => onSave(slot, { effort })}
        />
      </div>
    {/each}
  </div>
</div>
