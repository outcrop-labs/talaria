<script lang="ts">
  import { Search } from '@lucide/svelte'
  import Input from '@/components/ui/Input.svelte'
  import { popPanel } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { searchKb, type KbSearchHit } from '@/lib/kb'

  let { onOpen }: { onOpen: (hit: KbSearchHit) => void } = $props()

  let q = $state('')
  let hits = $state<KbSearchHit[]>([])
  let open = $state(false)
  $effect(() => {
    const t = q.trim()
    if (!t) {
      hits = []
      return
    }
    let live = true
    const id = setTimeout(() => {
      void searchKb(t).then((h) => {
        if (live) hits = h
      })
    }, 180)
    return () => {
      live = false
      clearTimeout(id)
    }
  })
</script>

<div class="relative">
  <Search size={13} class="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted" />
  <Input
    size="sm"
    bind:value={q}
    oninput={() => (open = true)}
    onfocus={() => (open = true)}
    onblur={() => setTimeout(() => (open = false), 150)}
    placeholder="Search knowledge"
    class="pl-7"
  />
  {#if open && hits.length > 0}
    <div
      in:pop={POPOVER}
      out:fade={QUICK}
      class={cn(popPanel, 'absolute left-0 right-0 top-full z-20 mt-1 max-h-80 origin-top overflow-y-auto')}
    >
      {#each hits as h (h.id)}
        <button
          type="button"
          onmousedown={(e) => {
            e.preventDefault()
            onOpen(h)
            open = false
          }}
          class="flex w-full flex-col gap-0.5 rounded-md px-2 py-1.5 text-left transition-colors dither-fill"
        >
          <span class="flex items-center gap-1.5 text-xs text-fg">
            <span class="w-4 shrink-0 text-center">{h.icon ?? '📄'}</span>
            <span class="truncate font-medium">{h.title}</span>
            <span class="ml-auto shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{h.spaceName}</span>
          </span>
          {#if h.snippet}<span class="line-clamp-2 text-[11px] text-muted">{@html h.snippet}</span>{/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
