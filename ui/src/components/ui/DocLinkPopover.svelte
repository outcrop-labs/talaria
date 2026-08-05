<script lang="ts">
  import { cn } from '@/lib/cn'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { popPanel } from '@/components/chat/chat-chrome'
  import Input from './Input.svelte'
  import type { DocSearchFn } from './rich-editor'

  // Fuzzy doc-picker popover for cross-references. Debounced search over the
  // caller-provided function; click a result to insert a link.
  let {
    search,
    onPick,
    onClose,
  }: {
    search: DocSearchFn
    onPick: (doc: { title: string; icon?: string | null; href: string }) => void
    onClose: () => void
  } = $props()

  let ref = $state<HTMLDivElement | null>(null)
  let q = $state('')
  let results = $state<Array<{ id: string; title: string; icon?: string | null; href: string }>>([])

  // Close on any mousedown outside the popover.
  $effect(() => {
    const h = (e: MouseEvent) => {
      if (ref && !ref.contains(e.target as Node)) onClose()
    }
    document.addEventListener('mousedown', h)
    return () => document.removeEventListener('mousedown', h)
  })

  // Debounced search — also fires once on open with the empty query.
  $effect(() => {
    const query = q
    let live = true
    const id = setTimeout(() => {
      void search(query).then((r) => {
        if (live) results = r
      })
    }, 160)
    return () => {
      live = false
      clearTimeout(id)
    }
  })
</script>

<!-- |global: the panel IS the component root — EditorToolbar renders
     {#if docLinkOpen}<DocLinkPopover/>, so local legs never play (ANIMATIONS.md). -->
<div
  bind:this={ref}
  in:pop|global={POPOVER}
  out:fade|global={QUICK}
  class={cn('absolute left-0 top-full z-30 mt-1 w-72 origin-top-left', popPanel)}
>
  <!-- svelte-ignore a11y_autofocus -->
  <Input autofocus size="sm" bind:value={q} placeholder="Search documents" class="mb-1.5" />
  <div class="max-h-64 overflow-y-auto">
    {#if results.length === 0}
      <div class="px-2 py-3 text-center text-xs text-muted">{q.trim() ? 'No matches.' : 'Type to search docs.'}</div>
    {:else}
      {#each results as d (d.id)}
        <button
          type="button"
          onmousedown={(e) => e.preventDefault()}
          onclick={() => onPick(d)}
          class="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-left font-sans text-[13px] text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          <span>{d.icon ?? '📄'}</span>
          <span class="min-w-0 flex-1 truncate">{d.title}</span>
        </button>
      {/each}
    {/if}
  </div>
</div>
