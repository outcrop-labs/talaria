<script lang="ts">
  import KbDocRow from './KbDocRow.svelte'
  import { Bot, ChevronRight, FileText, Plus, Star } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import type { KbDocMeta } from '@/lib/kb'

  type DropPos = 'before' | 'after' | 'inside'

  let {
    doc,
    depth,
    byParent,
    activeId,
    onSelect,
    onNew,
    dragId,
    setDragId,
    onDropRel,
    onDocMenu,
  }: {
    doc: KbDocMeta
    depth: number
    byParent: Map<string | null, KbDocMeta[]>
    activeId: string | null
    onSelect: (id: string) => void
    onNew: (k: 'human' | 'agent', parentId?: string | null) => void
    dragId: string | null
    setDragId: (id: string | null) => void
    onDropRel: (targetId: string, pos: DropPos) => void
    onDocMenu: (e: MouseEvent, d: KbDocMeta) => void
  } = $props()

  const kids = $derived(byParent.get(doc.id) ?? [])
  let expanded = $state(true)
  let pos = $state<DropPos | null>(null)
  const hasKids = $derived(kids.length > 0)
</script>

<div>
  <div
    draggable="true"
    ondragstart={(e) => {
      e.stopPropagation()
      setDragId(doc.id)
      e.dataTransfer!.effectAllowed = 'move'
    }}
    ondragend={() => setDragId(null)}
    ondragover={(e) => {
      e.preventDefault()
      e.stopPropagation()
      if (!dragId || dragId === doc.id) return
      // Top third → before, bottom third → after, middle → nest inside.
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
      const y = (e.clientY - r.top) / r.height
      pos = y < 0.3 ? 'before' : y > 0.7 ? 'after' : 'inside'
    }}
    ondragleave={() => (pos = null)}
    ondrop={(e) => {
      e.preventDefault()
      e.stopPropagation()
      if (pos) onDropRel(doc.id, pos)
      pos = null
      setDragId(null)
    }}
    oncontextmenu={(e) => onDocMenu(e, doc)}
    class={cn(
      'group relative flex items-center gap-1 rounded-md py-1 pr-1 text-xs transition-colors',
      activeId === doc.id ? 'bg-card text-fg' : 'text-muted dither-fill hover:text-fg',
      pos === 'inside' && 'ring-1 ring-accent/60',
    )}
    style:padding-left="{depth * 12 + 4}px"
    role="presentation"
  >
    {#if pos === 'before'}<span class="absolute inset-x-1 top-0 h-0.5 rounded bg-accent"></span>{/if}
    {#if pos === 'after'}<span class="absolute inset-x-1 bottom-0 h-0.5 rounded bg-accent"></span>{/if}
    <button
      type="button"
      onclick={() => (expanded = !expanded)}
      class={cn('shrink-0 rounded p-0.5 hover:bg-card2', !hasKids && 'invisible')}
    >
      <ChevronRight size={12} class={cn('transition-transform', expanded && 'rotate-90')} />
    </button>
    <button type="button" onclick={() => onSelect(doc.id)} class="flex min-w-0 flex-1 items-center gap-1.5 text-left">
      <!-- Fixed icon lane (§8): emoji and lucide icons share one slot. -->
      <span class="grid w-4 shrink-0 place-items-center">
        {#if doc.icon}<span class="text-[13px] leading-none">{doc.icon}</span>{:else if doc.kind === 'agent'}<Bot size={12} />{:else}<FileText size={12} />{/if}
      </span>
      <span class="min-w-0 flex-1 truncate">{doc.title}</span>
      {#if doc.official}<Star size={11} class="shrink-0 text-warning" />{/if}
    </button>
    <button
      type="button"
      onclick={(e) => {
        e.stopPropagation()
        onNew('human', doc.id)
        expanded = true
      }}
      title="New nested doc"
      class="shrink-0 rounded p-0.5 text-muted opacity-0 hover:bg-card2 hover:text-fg group-hover:opacity-100"
    >
      <Plus size={12} />
    </button>
  </div>
  {#if hasKids && expanded}
    <div>
      {#each kids as k (k.id)}
        <KbDocRow
          doc={k}
          depth={depth + 1}
          {byParent}
          {activeId}
          {onSelect}
          {onNew}
          {dragId}
          {setDragId}
          {onDropRel}
          {onDocMenu}
        />
      {/each}
    </div>
  {/if}
</div>
