<script lang="ts">
  import { SlidersHorizontal, GripVertical } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { flip, LIST } from '@/lib/motion'
  import { popHeader } from '@/components/chat/chat-chrome'
  import Popover from '@/components/ui/Popover.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import { LIST_COLUMNS, type ColumnKey } from './board-list'

  let {
    visible,
    order,
    onChangeVisible,
    onChangeOrder,
  }: {
    visible: ColumnKey[]
    order: ColumnKey[]
    onChangeVisible: (c: ColumnKey[]) => void
    onChangeOrder: (c: ColumnKey[]) => void
  } = $props()

  let dragKey = $state<ColumnKey | null>(null)
  let overKey = $state<ColumnKey | null>(null)
  let overPos = $state<'before' | 'after'>('before')

  const toggle = (k: ColumnKey) =>
    onChangeVisible(visible.includes(k) ? visible.filter((x) => x !== k) : [...visible, k])

  // Drop the dragged column before/after the row, based on where it was released.
  const drop = (target: ColumnKey) => {
    if (dragKey && dragKey !== target) {
      const next = order.filter((k) => k !== dragKey)
      const idx = next.indexOf(target) + (overPos === 'after' ? 1 : 0)
      next.splice(idx, 0, dragKey)
      onChangeOrder(next)
    }
    dragKey = null
    overKey = null
  }
</script>

<!-- Icon-only trigger living in the table's trailing header cell; the §7
     popover shell owns the panel (portal, fixed, Esc/scroll close) so the
     scroll container can't clip it. -->
<Popover align="right" class="w-48">
  {#snippet trigger(open)}
    <button
      title="Columns: show, hide, reorder"
      aria-label="Configure columns"
      aria-expanded={open}
      class={cn(
        'grid h-6 w-6 place-items-center rounded-md transition-colors',
        open ? 'bg-raised text-fg' : 'text-muted dither-fill hover:text-fg',
      )}
    >
      <SlidersHorizontal size={13} />
    </button>
  {/snippet}
  {#snippet content()}
    <div class={popHeader}>Drag to reorder</div>
    {#each order as key (key)}
      {@const c = LIST_COLUMNS.find((x) => x.key === key)!}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        animate:flip={LIST}
        draggable="true"
        ondragstart={() => (dragKey = c.key)}
        ondragend={() => {
          dragKey = null
          overKey = null
        }}
        ondragover={(e) => {
          e.preventDefault()
          const rect = e.currentTarget.getBoundingClientRect()
          overKey = c.key
          overPos = e.clientY > rect.top + rect.height / 2 ? 'after' : 'before'
        }}
        ondrop={(e) => {
          e.preventDefault()
          drop(c.key)
        }}
        class={cn(
          'relative flex items-center gap-1.5 rounded-md px-1.5 py-1.5 font-sans text-[13px] text-fg transition-colors dither-fill',
          dragKey === c.key && 'opacity-40',
        )}
      >
        {#if overKey === c.key && dragKey !== c.key}
          <span
            class={cn(
              'pointer-events-none absolute inset-x-1 h-0.5 rounded-full bg-accent',
              overPos === 'before' ? '-top-px' : '-bottom-px',
            )}
          ></span>
        {/if}
        <GripVertical size={13} class="shrink-0 cursor-grab text-muted active:cursor-grabbing" />
        <Checkbox
          bare
          title={c.label}
          checked={c.fixed || visible.includes(c.key)}
          disabled={c.fixed}
          onChange={() => toggle(c.key)}
        />
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <span class={cn('cursor-pointer select-none', c.fixed && 'opacity-60')} onclick={() => !c.fixed && toggle(c.key)}>
          {c.label}
        </span>
      </div>
    {/each}
  {/snippet}
</Popover>
