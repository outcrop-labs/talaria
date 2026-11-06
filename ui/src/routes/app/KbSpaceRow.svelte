<script lang="ts">
  import { ChevronRight } from '@lucide/svelte'
  import Input from '@/components/ui/Input.svelte'
  import { cn } from '@/lib/cn'
  import { kbTreeState } from '@/lib/knowledge-tree-state.svelte'
  import type { KbSpace } from '@/lib/kb'

  let {
    space,
    active,
    onSelect,
    onRename,
    onContextMenu,
  }: {
    space: KbSpace
    active: boolean
    onSelect: () => void
    onRename: (name: string) => void
    onContextMenu?: (e: MouseEvent) => void
  } = $props()

  // The chevron is the SECTION toggle (TALA-92); clicking the name still
  // selects the space and drives the main pane. Unlike KbDocRow's chevron
  // this one is always visible: "has docs" is not cheaply knowable here —
  // the doc list is per-space and only the active space's is loaded — and
  // an empty section is a real state, not a broken one. No local state: the
  // store is $state-backed in a .svelte.ts module, so reading it in the
  // template is reactive, and Knowledge.svelte's tree gate reads the same
  // method — one source of truth for the row and the tree.
  const sectionOpen = () => kbTreeState.sectionExpanded(space.id)

  let editing = $state(false)
  // svelte-ignore state_referenced_locally -- reason: seed-once draft; the $effect below re-syncs it on every space change
  let name = $state(space.name)
  $effect(() => {
    name = space.name
  })
</script>


{#if editing}
  <Input
    size="sm"
    autofocus
    bind:value={name}
    onblur={() => {
      editing = false
      if (name.trim() && name !== space.name) onRename(name.trim())
    }}
    onkeydown={(e) => {
      if (e.key === 'Enter') e.currentTarget.blur()
      if (e.key === 'Escape') {
        name = space.name
        editing = false
      }
    }}
  />
{:else}
  <div
    class={cn('group flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors', active ? 'bg-card text-fg' : 'text-muted dither-fill hover:text-fg')}
    oncontextmenu={onContextMenu}
    role="presentation"
  >
    <button
      type="button"
      onclick={() => kbTreeState.toggleSection(space.id, !sectionOpen())}
      title={sectionOpen() ? 'Collapse section' : 'Expand section'}
      class="shrink-0 rounded p-0.5 hover:bg-card2"
    >
      <ChevronRight size={12} class={cn('transition-transform', sectionOpen() && 'rotate-90')} />
    </button>
    <button type="button" onclick={onSelect} ondblclick={() => (editing = true)} class="flex min-w-0 flex-1 items-center gap-2 text-left">
      <!-- Fixed icon lane so titles align across rows (§8 list pattern). -->
      <span class="w-4 shrink-0 text-center">{space.icon ?? '📚'}</span>
      <span class="truncate font-medium">{space.name}</span>
    </button>
  </div>
{/if}
