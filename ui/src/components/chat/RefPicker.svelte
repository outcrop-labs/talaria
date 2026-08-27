<script lang="ts">
  import { BookOpen, Gem } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { getJson } from '@/lib/fetch-json'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { popRow } from '@/components/chat/chat-chrome'
  import { searchKb } from '@/lib/kb'
  import type { Attachment } from '@/lib/attachments'

  // Search-and-pick for knowledge docs / artifacts inside the attach menu
  // (AttachButton.svelte's second pane).
  let { kind, onPick }: { kind: 'kb-doc' | 'artifact'; onPick: (a: Attachment) => void } = $props()

  let q = $state('')
  let results = $state<Array<{ id: string; title: string }>>([])
  let loading = $state(false)

  $effect(() => {
    // Read the runes up front so the debounced closure tracks them.
    const query = q
    const k = kind
    let cancelled = false
    const t = setTimeout(async () => {
      loading = true
      try {
        if (k === 'kb-doc') {
          const hits = query.trim() ? await searchKb(query) : []
          if (!cancelled) results = hits.slice(0, 8).map((h) => ({ id: h.id, title: h.title || 'Untitled' }))
        } else {
          const all = (await getJson<{ artifacts?: Array<{ id: string; title: string }> }>('/api/artifacts')).artifacts ?? []
          const needle = query.trim().toLowerCase()
          if (!cancelled) {
            results = all
              .filter((a) => !needle || a.title.toLowerCase().includes(needle))
              .slice(0, 8)
              .map((a) => ({ id: a.id, title: a.title || 'Untitled' }))
          }
        }
      } finally {
        if (!cancelled) loading = false
      }
    }, 200)
    return () => {
      cancelled = true
      clearTimeout(t)
    }
  })
</script>

<div class="p-1">
  <!-- Popover search row (spec §7 popover pattern: mono placeholder, ⌘K hint). -->
  <PopSearch value={q} onChange={(v) => (q = v)} placeholder={kind === 'kb-doc' ? 'Search knowledge' : 'Search artifacts'} />
  <div class="max-h-48 overflow-y-auto">
    {#if loading}
      <SkeletonRows rows={3} class="px-2 py-2" />
    {:else if results.length === 0}
      <div class="px-2 py-1.5 text-xs text-muted">{kind === 'kb-doc' && !q.trim() ? 'Type to search docs' : 'No matches'}</div>
    {/if}
    {#each results as r (r.id)}
      <button
        type="button"
        onclick={() => onPick({ id: r.id, filename: r.title, mime: `ref/${kind}`, size: 0, refType: kind })}
        class={cn(popRow, 'text-[13px] text-fg')}
      >
        {#if kind === 'kb-doc'}
          <BookOpen size={13} class="shrink-0 text-muted" />
        {:else}
          <Gem size={13} class="shrink-0 text-muted" />
        {/if}
        <span class="min-w-0 flex-1 truncate">{r.title}</span>
      </button>
    {/each}
  </div>
</div>
