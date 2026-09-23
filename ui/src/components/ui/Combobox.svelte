<script lang="ts" module>
  // Module-scope counter: every instance's listbox id is unique without
  // dragging in crypto or a global registry.
  let seq = 0
</script>

<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import { popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import { controlSizes, type ControlSize } from './control'
  import { fuzzy, type ComboOption } from './combobox'

  // A searchable dropdown for picking option(s). Fuzzy filter as you type.
  // Single mode: selecting closes + reports the value. Multi: toggles, stays open.
  // Keyboard: ↑/↓/Home/End walk a highlight through the filtered rows, Enter
  // activates it, Tab closes without picking. The Create row (allowCreate)
  // is never arrow-highlighted — click it or press comma to commit the text.
  let {
    options,
    selected,
    onChange,
    multiple = false,
    placeholder = 'Select',
    disabled,
    class: className,
    size = 'md',
    allowCreate = false,
    searchable = true,
    bare = false,
    triggerLabel,
  }: {
    options: ComboOption[]
    selected: string[]
    onChange: (next: string[]) => void
    multiple?: boolean
    placeholder?: string
    disabled?: boolean
    class?: string
    size?: ControlSize
    allowCreate?: boolean
    /** Show the search field. Off for short option lists (a clean menu). */
    searchable?: boolean
    /** Trigger without its own frame — for a picker embedded as one segment
     *  of a bordered control cluster (e.g. the [effort | model] assignment
     *  cluster), the role a transparent <Select> used to play there. */
    bare?: boolean
    /** Override the trigger content (e.g. a constant "Add label" for tag inputs). */
    triggerLabel?: string | Snippet
  } = $props()

  let open = $state(false)
  let q = $state('')
  let ref = $state<HTMLDivElement | null>(null)
  let panelRef = $state<HTMLDivElement | null>(null)
  // The panel renders in a body portal (fixed) — ancestors like .mercury-panel
  // create stacking contexts (backdrop-filter), which would cage an absolute
  // dropdown's z-index below the next card. Flips upward near the bottom edge.
  let pos = $state<{ top?: number; bottom?: number; left: number; width: number } | null>(null)

  // Keyboard highlight: an index into `filtered` (the Create row is never in
  // it — arrows skip it by construction). −1 = nothing highlighted.
  let hl = $state(-1)
  const listId = `combo-list-${seq++}`

  $effect(() => {
    if (!open) return
    const place = () => {
      const r = ref?.getBoundingClientRect()
      if (!r) return
      const spaceBelow = window.innerHeight - r.bottom
      const flip = spaceBelow < 320 && r.top > spaceBelow
      pos = flip
        ? { bottom: window.innerHeight - r.top + 4, left: r.left, width: r.width }
        : { top: r.bottom + 4, left: r.left, width: r.width }
    }
    place()
    const onClick = (e: MouseEvent) => {
      const t = e.target as Node
      if (!ref?.contains(t) && !panelRef?.contains(t)) open = false
    }
    document.addEventListener('mousedown', onClick)
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      document.removeEventListener('mousedown', onClick)
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  })

  // The highlight re-enters at the top of whatever the filter shows — both
  // a reopen and every keystroke reset it to the first row.
  $effect(() => {
    if (!open) return
    hl = filtered.length > 0 ? 0 : -1
  })

  // Focus the search on open. The input's native `autofocus` only fires when
  // body holds focus (Svelte's own rule); a trigger CLICK leaves focus on the
  // button, so an explicit focus is what makes typing work on open.
  $effect(() => {
    if (!open) return
    panelRef?.querySelector<HTMLInputElement>('input')?.focus()
  })

  // Keep the highlighted row scrolled into view — the ul is the scroll box.
  $effect(() => {
    if (!open || hl < 0) return
    panelRef?.querySelector('[data-hl]')?.scrollIntoView({ block: 'nearest' })
  })

  const filtered = $derived(options.filter((o) => fuzzy(q, o.label + ' ' + (o.sub ?? ''))))
  const selectedSet = $derived(new Set(selected))
  const byValue = (v: string) => options.find((o) => o.value === v)

  const toggle = (v: string) => {
    if (multiple) onChange(selectedSet.has(v) ? selected.filter((x) => x !== v) : [...selected, v])
    else {
      onChange([v])
      open = false
    }
  }

  // A trimmed query that matches no existing option can be created.
  const creatable = $derived(allowCreate ? q.trim().replace(/,+$/, '') : '')
  const canCreate = $derived(
    !!creatable && !options.some((o) => o.value.toLowerCase() === creatable.toLowerCase()) && !selectedSet.has(creatable),
  )

  const create = () => {
    if (!canCreate) return
    toggle(creatable)
    q = ''
  }

  // One keydown handler for both focus homes: the search input (searchable)
  // and the trigger button (searchable={false} renders no input, so focus
  // stays on the trigger and the keys arrive there).
  const onKeydown = (e: KeyboardEvent) => {
    if (!open) return
    if (e.key === 'Escape') {
      // Close just the dropdown — without this, the event reaches the Modal's
      // document-level listener and closes the whole dialog (losing edits).
      e.preventDefault()
      e.stopPropagation()
      open = false
      // Keyboard close hands focus back — without this it falls to <body>
      // and the next Enter/arrows go nowhere.
      ref?.querySelector('button')?.focus()
      return
    }
    // Tab moves focus on — close behind it, activate nothing. Focus first
    // returns to the trigger so the default Tab action advances from there.
    if (e.key === 'Tab') {
      open = false
      ref?.querySelector('button')?.focus()
      return
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      const n = filtered.length
      if (n === 0) return
      const d = e.key === 'ArrowDown' ? 1 : -1
      const from = hl >= 0 && hl < n ? hl : d > 0 ? -1 : 0
      hl = (from + d + n) % n
      return
    }
    if (e.key === 'Home' || e.key === 'End') {
      e.preventDefault()
      if (filtered.length > 0) hl = e.key === 'Home' ? 0 : filtered.length - 1
      return
    }
    if (e.key === 'Enter' && !e.isComposing) {
      e.preventDefault()
      // The highlighted row wins; with nothing highlighted (no matches) an
      // allowCreate combobox commits the typed text instead.
      const o = filtered[hl]
      if (o) toggle(o.value)
      else create()
      return
    }
    if (allowCreate && e.key === ',' && !e.isComposing) {
      e.preventDefault()
      create()
    }
  }
</script>

<div bind:this={ref} class={cn('relative', className)}>
  <button
    type="button"
    {disabled}
    onclick={() => (open = !open)}
    aria-haspopup="listbox"
    aria-expanded={open}
    onkeydown={(e) => {
      // Arrows on a closed combobox open it — the closed half of the walk.
      if (!open && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
        e.preventDefault()
        open = true
      } else onKeydown(e)
    }}
    class={cn(
      controlSizes[size],
      'flex w-full items-center gap-2 rounded-md font-sans text-sm outline-none transition-colors disabled:opacity-50',
      bare
        ? // A cluster segment: the cluster owns the frame, so the trigger
          // draws no border of its own and only surfaces on hover/focus.
          'border border-transparent bg-transparent px-2 hover:bg-card/60 focus-visible:ring-2 focus-visible:ring-accent-soft'
        : // Spec §8 input pattern: raised tile, hairline, radius 6, gold focus.
          'border border-line bg-[var(--theme-input)] px-2.5 hover:border-line-strong focus-visible:border-accent focus-visible:ring-2 focus-visible:ring-accent-soft',
    )}
  >
    <span class="min-w-0 flex-1 text-left">
      {#if triggerLabel !== undefined}
        {#if typeof triggerLabel === 'string'}{triggerLabel}{:else}{@render triggerLabel()}{/if}
      {:else if selected.length === 0}
        <span class="text-muted">{placeholder}</span>
      {:else if !multiple}
        {@const o = byValue(selected[0]!)}
        <span class="flex min-w-0 items-center gap-2">
          {@render o?.icon?.()}
          <span class="truncate text-fg">{o?.label ?? selected[0]}</span>
        </span>
      {:else}
        {@const labels = selected.map((v) => byValue(v)?.label ?? v)}
        <span class="truncate text-fg">
          {labels.slice(0, 2).join(', ')}{labels.length > 2 ? ` +${labels.length - 2}` : ''}
        </span>
      {/if}
    </span>
    <span class="text-muted">▾</span>
  </button>

  {#if open && pos}
    <div
      use:portal
      bind:this={panelRef}
      in:pop={POPOVER}
      out:fade={QUICK}
      class={cn(
        'fixed z-[60] rounded-[10px] border border-line bg-panel p-1 shadow-[var(--theme-shadow-2)]',
        pos.bottom !== undefined ? 'origin-bottom' : 'origin-top',
      )}
      style:top={pos.top !== undefined ? `${pos.top}px` : undefined}
      style:bottom={pos.bottom !== undefined ? `${pos.bottom}px` : undefined}
      style:left={`${pos.left}px`}
      style:width={`${pos.width}px`}
    >
      <!-- Spec §7 popover pattern: panel bg, hairline, radius 10,
           search field on top, hover card2, selected = dashed gold. -->
      {#if searchable || allowCreate}
        <div class="mb-1 flex items-center gap-1.5 rounded-md border border-line px-2">
          <!-- svelte-ignore a11y_autofocus -->
          <input
            autofocus
            bind:value={q}
            onkeydown={onKeydown}
            placeholder={allowCreate ? 'Search or create' : 'Search'}
            aria-controls={listId}
            aria-activedescendant={filtered[hl] ? `${listId}-${hl}` : undefined}
            class="h-7 w-full min-w-0 bg-transparent font-mono text-[11px] tracking-[0.05em] text-fg outline-none placeholder:text-muted"
          />
        </div>
      {/if}
      <ul id={listId} role="listbox" class="max-h-56 overflow-y-auto">
        {#if filtered.length === 0 && !canCreate}
          <li role="presentation" class="px-2 py-2 font-mono text-[11px] text-muted">No matches</li>
        {/if}
        {#if canCreate}
          <li role="presentation">
            <button type="button" onclick={create} class={cn(popRow, 'text-fg')}>
              <span class="text-accent">＋</span> Create “{creatable}”
            </button>
          </li>
        {/if}
        {#each filtered as o, i (o.value)}
          <li role="presentation">
            <button
              type="button"
              role="option"
              aria-selected={selectedSet.has(o.value)}
              id={`${listId}-${i}`}
              data-hl={hl === i || undefined}
              onclick={() => toggle(o.value)}
              class={cn(popRow, hl === i && 'bg-hover', selectedSet.has(o.value) && popRowSelected)}
            >
              {@render o.icon?.()}
              <span class="min-w-0 flex-1">
                <span class="block truncate text-fg">{o.label}</span>
                {#if o.sub}
                  <span class="block truncate font-mono text-[10px] tracking-[0.05em] text-muted">{o.sub}</span>
                {/if}
              </span>
              {#if selectedSet.has(o.value)}<span class="text-accent">✓</span>{/if}
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>
