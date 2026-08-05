<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { Artifact } from '@/lib/artifacts'
  import { KIND_ICON, type Drag } from './artifacts'

  let {
    artifact: a,
    depth,
    activeId,
    onSelect,
    setDrag,
    onContextMenu,
  }: {
    artifact: Artifact
    depth: number
    activeId: string | null
    onSelect: (id: string) => void
    setDrag: (d: Drag) => void
    onContextMenu?: (e: MouseEvent) => void
  } = $props()

  const Icon = $derived(KIND_ICON[a.kind])
</script>

<button
  type="button"
  draggable="true"
  ondragstart={(e) => {
    e.stopPropagation()
    setDrag({ kind: 'artifact', id: a.id })
  }}
  ondragend={() => setDrag(null)}
  onclick={() => onSelect(a.id)}
  oncontextmenu={onContextMenu}
  class={cn('flex w-full items-center gap-2 rounded-md py-1.5 pr-2 text-left text-sm transition-colors', activeId === a.id ? 'bg-card text-fg' : 'text-muted hover:bg-hover hover:text-fg')}
  style:padding-left="{depth * 14 + 8}px"
>
  <!-- Fixed icon lane (§8) — emoji and lucide share one slot. -->
  <span class="grid w-4 shrink-0 place-items-center">
    {#if a.icon}<span class="text-[15px] leading-none">{a.icon}</span>{:else}<Icon size={14} />{/if}
  </span>
  <span class="min-w-0 flex-1 truncate">{a.title}</span>
  <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{a.kind}</span>
</button>
