<script lang="ts">
  import { cn } from '@/lib/cn'
  import { focusGold, popPanel, popRow } from '@/components/chat/chat-chrome'
  import { fade, scale, POP, QUICK } from '@/lib/motion'
  import Avatar from '@/components/ui/Avatar.svelte'

  // A neat little header multiselect: a pill ("3 people ▾") opening a checklist
  // popover. Toggles apply immediately; locked rows (owners) can't be removed.
  let {
    label,
    options,
    selected,
    onToggle,
  }: {
    label: string
    options: { value: string; label: string; locked?: boolean }[]
    selected: string[]
    onToggle: (value: string, on: boolean) => Promise<void>
  } = $props()

  let open = $state(false)
  const picked = $derived(new Set(selected))
  const chosen = $derived(options.filter((o) => picked.has(o.value)))
</script>

<div class="relative shrink-0">
  <button
    type="button"
    onclick={() => (open = !open)}
    class={cn(
      'flex items-center gap-1.5 rounded-md border border-line px-2 py-1 text-xs transition-colors hover:bg-hover hover:text-fg',
      focusGold,
      open ? 'bg-raised text-fg' : 'text-muted',
    )}
  >
    <!-- Truncated avatar stack — fixed width so the header never jiggles. -->
    <span class="flex w-11 shrink-0 justify-start -space-x-2">
      {#each chosen.slice(0, 3) as o (o.value)}
        <Avatar name={o.label} class="h-5 w-5 shrink-0 text-[9px] ring-2 ring-surface" />
      {/each}
      {#if chosen.length > 3}
        <span class="grid h-5 w-5 shrink-0 place-items-center rounded-full bg-raised text-[9px] text-muted ring-2 ring-surface">
          +{chosen.length - 3}
        </span>
      {/if}
      {#if chosen.length === 0}
        <span class="grid h-5 w-5 shrink-0 place-items-center rounded-full border border-dashed border-line text-[9px] text-muted">
          +
        </span>
      {/if}
    </span>
    {label} <span class="opacity-60">▾</span>
  </button>
  {#if open}
    <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
    <div class="fixed inset-0 z-10" onclick={() => (open = false)}></div>
    <div
      in:scale={{ ...POP, start: 0.97 }}
      out:fade={QUICK}
      class={cn(popPanel, 'absolute right-0 top-full z-20 mt-1 max-h-64 w-56 origin-top-right overflow-y-auto')}
    >
      {#if options.length === 0}<div class="px-2 py-1.5 text-xs text-muted">Nothing to add.</div>{/if}
      {#each options as o (o.value)}
        {@const on = picked.has(o.value)}
        <button
          type="button"
          disabled={o.locked}
          title={o.locked ? 'The owner stays' : undefined}
          onclick={() => void onToggle(o.value, !on)}
          class={cn(
            popRow,
            o.locked && 'cursor-default opacity-60 hover:bg-transparent',
            on ? 'text-fg' : 'text-muted',
          )}
        >
          <Avatar name={o.label} class="h-5 w-5 shrink-0 text-[10px]" />
          <span class="min-w-0 flex-1 truncate">{o.label}</span>
          {#if on}<span class="shrink-0 text-success">✓</span>{/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
