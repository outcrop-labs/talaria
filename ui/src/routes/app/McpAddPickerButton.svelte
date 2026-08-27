<script lang="ts">
  import { Plus } from '@lucide/svelte'
  import Input from '@/components/ui/Input.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import { popRow } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'

  /** A "+" that opens a search popover anchored to itself — pick to commit.
   *  The attach-menu pattern: outside click or Esc dismisses. */
  let {
    title,
    placeholder,
    options,
    onPick,
  }: {
    title: string
    placeholder: string
    options: Array<{ value: string; label: string; sub?: string }>
    onPick: (value: string) => void
  } = $props()

  let q = $state('')

  const results = $derived.by(() => {
    const needle = q.trim().toLowerCase()
    return options.filter((o) => !needle || o.label.toLowerCase().includes(needle) || (o.sub ?? '').toLowerCase().includes(needle))
  })
</script>

<Popover align="right" class="w-64 p-1.5">
  {#snippet trigger(open)}
    <!-- The click itself is the popover's (the shell toggles); this handler
         only clears the search so every open starts fresh, as the old
         toggle-button reset did. -->
    <button
      type="button"
      {title}
      onclick={() => (q = '')}
      class={cn(
        'grid h-6 w-6 place-items-center rounded-md transition-colors',
        open ? 'bg-raised text-accent' : 'text-muted dither-fill hover:text-accent',
      )}
    >
      <Plus size={14} />
    </button>
  {/snippet}
  {#snippet content(close)}
    <Input autofocus size="sm" bind:value={q} {placeholder} class="mb-1" />
    <div class="max-h-48 overflow-y-auto">
      {#if results.length === 0}<div class="px-2 py-1.5 text-xs text-muted">No matches</div>{/if}
      {#each results as o (o.value)}
        <button
          type="button"
          onclick={() => {
            close()
            onPick(o.value)
          }}
          class={cn(popRow, 'items-baseline')}
        >
          <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{o.label}</span>
          {#if o.sub}<span class="shrink-0 truncate font-mono text-[11px] text-muted">{o.sub}</span>{/if}
        </button>
      {/each}
    </div>
  {/snippet}
</Popover>
