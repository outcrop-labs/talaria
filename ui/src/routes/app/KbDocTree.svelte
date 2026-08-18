<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { Bot, Plus } from '@lucide/svelte'
  import { moveDoc, type KbDocMeta } from '@/lib/kb'
  import KbDocRow from './KbDocRow.svelte'

  type DropPos = 'before' | 'after' | 'inside'

  let {
    docs,
    activeId,
    onSelect,
    onNew,
    onMove,
    onDocMenu,
  }: {
    docs: KbDocMeta[]
    activeId: string | null
    onSelect: (id: string) => void
    onNew: (k: 'human' | 'agent', parentId?: string | null) => void
    onMove: (id: string, parentId: string | null, sort: number) => void
    onDocMenu: (e: MouseEvent, d: KbDocMeta) => void
  } = $props()

  const byParent = $derived.by(() => {
    const m = new Map<string | null, KbDocMeta[]>()
    for (const d of docs) {
      const k = d.parentId ?? null
      if (!m.has(k)) m.set(k, [])
      m.get(k)!.push(d)
    }
    for (const list of m.values()) list.sort((a, b) => a.sort - b.sort || a.title.localeCompare(b.title))
    return m
  })

  let dragId = $state<string | null>(null)

  // Reparent/reorder: drop 'inside' T nests under it; 'before'/'after' T orders
  // among T's siblings. Sibling order is persisted by reindexing that group.
  const drop = (targetId: string, pos: DropPos) => {
    if (!dragId || dragId === targetId) return
    const target = docs.find((d) => d.id === targetId)
    if (!target) return
    if (pos === 'inside') {
      const kids = byParent.get(targetId) ?? []
      onMove(dragId, targetId, kids.length)
      return
    }
    const parent = target.parentId ?? null
    const sibs = (byParent.get(parent) ?? []).filter((d) => d.id !== dragId)
    const idx = sibs.findIndex((d) => d.id === targetId)
    const at = pos === 'before' ? idx : idx + 1
    sibs.splice(at, 0, docs.find((d) => d.id === dragId)!)
    sibs.forEach((d, i) => {
      if (d.id === dragId) onMove(dragId, parent, i)
      else if (d.sort !== i) void moveDoc(d.id, parent, i)
    })
  }

  const roots = $derived(byParent.get(null) ?? [])
</script>

<div
  class="ml-2 mt-0.5 space-y-0.5 border-l border-line-subtle pl-2"
  ondragover={(e) => e.preventDefault()}
  ondrop={(e) => {
    // Drop in the empty tree area → move to root (append).
    e.preventDefault()
    if (dragId) onMove(dragId, null, roots.length)
    dragId = null
  }}
  role="tree"
  tabindex="-1"
>
  {#each roots as d (d.id)}
    <KbDocRow
      doc={d}
      depth={0}
      {byParent}
      {activeId}
      {onSelect}
      {onNew}
      {dragId}
      setDragId={(id) => (dragId = id)}
      onDropRel={drop}
      {onDocMenu}
    />
  {/each}
  <!-- Ghost actions (§8): mono uppercase, muted → readout. -->
  <div class="flex gap-1 pt-1">
    <Button variant="ghost" size="xs" class="gap-1 py-1" onclick={() => onNew('human')}>
      <Plus size={11} /> Doc
    </Button>
    <Button variant="ghost" size="xs" class="gap-1 py-1" onclick={() => onNew('agent')}  title="OKF-structured for agents">
      <Bot size={11} /> Agent doc
    </Button>
  </div>
</div>
