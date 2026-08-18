<script lang="ts">
  import Input from '@/components/ui/Input.svelte'
  import { cn } from '@/lib/cn'
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

  let editing = $state(false)
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
  <div data-dither-fill
    class={cn('group flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors', active ? 'bg-card text-fg' : 'text-muted hover:text-fg')}
    oncontextmenu={onContextMenu}
    role="presentation"
  >
    <button type="button" onclick={onSelect} ondblclick={() => (editing = true)} class="flex min-w-0 flex-1 items-center gap-2 text-left">
      <!-- Fixed icon lane so titles align across rows (§8 list pattern). -->
      <span class="w-4 shrink-0 text-center">{space.icon ?? '📚'}</span>
      <span class="truncate font-medium">{space.name}</span>
    </button>
  </div>
{/if}
