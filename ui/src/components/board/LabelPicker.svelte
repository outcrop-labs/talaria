<script lang="ts">
  import Combobox from '@/components/ui/Combobox.svelte'
  import type { ControlSize } from '@/components/ui/control'

  // Ticket labels as tag chips + the shared combobox: existing board tags surface
  // in the dropdown for reuse; typing creates a new one (Enter or comma commits).
  let {
    value,
    options,
    onChange,
    disabled,
    size,
  }: {
    value: string[]
    /** Tags already in use (e.g. across the board) — offered for reuse. */
    options: string[]
    onChange: (next: string[]) => void
    disabled?: boolean
    size?: ControlSize
  } = $props()

  const opts = $derived(
    [...new Set([...options, ...value])]
      .sort()
      .map((t) => ({ value: t, label: t })),
  )
</script>

<div class="space-y-1.5">
  {#if value.length > 0}
    <div class="flex flex-wrap gap-1">
      {#each value as t (t)}
        <span class="flex items-center gap-1 rounded-full border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em] text-muted">
          {t}
          {#if !disabled}
            <button
              type="button"
              aria-label={`Remove ${t}`}
              onclick={() => onChange(value.filter((x) => x !== t))}
              class="transition-colors hover:text-danger"
            >
              ✕
            </button>
          {/if}
        </span>
      {/each}
    </div>
  {/if}
  <Combobox options={opts} selected={value} {onChange} multiple allowCreate {disabled} {size}>
    {#snippet triggerLabel()}<span class="text-muted">Add label</span>{/snippet}
  </Combobox>
</div>
