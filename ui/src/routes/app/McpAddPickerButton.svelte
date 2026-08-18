<script lang="ts">
  import { Plus } from '@lucide/svelte'
  import Input from '@/components/ui/Input.svelte'
  import { popPanel, popRow } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'

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

  let open = $state(false)
  let q = $state('')
  let wrapRef = $state<HTMLDivElement | null>(null)

  $effect(() => {
    if (!open) return
    const onDoc = (e: MouseEvent) => {
      if (!wrapRef?.contains(e.target as Node)) open = false
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') open = false
    }
    document.addEventListener('mousedown', onDoc)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDoc)
      document.removeEventListener('keydown', onKey)
    }
  })

  const results = $derived.by(() => {
    const needle = q.trim().toLowerCase()
    return options.filter((o) => !needle || o.label.toLowerCase().includes(needle) || (o.sub ?? '').toLowerCase().includes(needle))
  })
</script>

<div bind:this={wrapRef} class="relative">
  <button data-dither-fill
    type="button"
    {title}
    onclick={() => {
      open = !open
      q = ''
    }}
    class={cn(
      'grid h-6 w-6 place-items-center rounded-md transition-colors',
      open ? 'bg-raised text-accent' : 'text-muted hover:text-accent',
    )}
  >
    <Plus size={14} />
  </button>
  {#if open}
    <div
      in:pop={POPOVER}
      out:fade={QUICK}
      class={cn(popPanel, 'absolute right-0 top-full z-30 mt-1 w-64 origin-top-right p-1.5')}
    >
      <Input autofocus size="sm" bind:value={q} {placeholder} class="mb-1" />
      <div class="max-h-48 overflow-y-auto">
        {#if results.length === 0}<div class="px-2 py-1.5 text-xs text-muted">No matches</div>{/if}
        {#each results as o (o.value)}
          <button
            type="button"
            onclick={() => {
              open = false
              onPick(o.value)
            }}
            class={cn(popRow, 'items-baseline')}
          >
            <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{o.label}</span>
            {#if o.sub}<span class="shrink-0 truncate font-mono text-[11px] text-muted">{o.sub}</span>{/if}
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>
