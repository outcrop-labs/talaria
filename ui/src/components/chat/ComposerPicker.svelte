<script lang="ts">
  // The composer's compact choice chip — the TierPicker pattern, generalized:
  // an icon + current value that opens a portaled popover. Use for anything a
  // composer needs decided at send time (model tier, research depth, acting
  // agent) instead of parking wide controls in rails or headers.
  //
  // Mercury (spec §7): a secondary 36px mono chip (hairline border, muted →
  // readout on hover) over the §7 popover pattern — search row with ⌘K hint,
  // panel bg, mono section header, right-aligned mono meta, hover fill,
  // dashed-gold selected row.
  import type { LucideIcon as IconType } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { portal } from '@/lib/portal'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipSecondary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import type { ComposerOption } from './composer-picker'

  let {
    icon: Icon,
    value,
    options,
    onChange,
    title,
    menuLabel,
    placement = 'top',
    class: className,
  }: {
    icon: IconType
    value: string
    options: ComposerOption[]
    onChange: (v: string) => void
    title: string
    menuLabel: string
    /** Which side of the trigger the popover anchors to. Composers sit at
     *  the bottom of their surface, so they open UP ('top'); a picker living
     *  in a header at the top of a stage opens DOWN, or the panel would fly
     *  off-screen. */
    placement?: 'top' | 'bottom'
    class?: string
  } = $props()

  let open = $state(false)
  let q = $state('')
  let pos = $state<{ left: number; top?: number; bottom?: number } | null>(null)
  let btnRef = $state<HTMLButtonElement | null>(null)
  let panelRef = $state<HTMLDivElement | null>(null)

  $effect(() => {
    if (!open) return
    const place = () => {
      const r = btnRef?.getBoundingClientRect()
      if (!r) return
      pos =
        placement === 'top'
          ? { left: r.left, bottom: window.innerHeight - r.top + 6 }
          : { left: r.left, top: r.bottom + 6 }
    }
    place()
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node
      if (!btnRef?.contains(t) && !panelRef?.contains(t)) open = false
    }
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    document.addEventListener('mousedown', onDoc)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      document.removeEventListener('mousedown', onDoc)
    }
  })

  const current = $derived(options.find((o) => o.value === value))
  const needle = $derived(q.trim().toLowerCase())
  const visible = $derived(
    options.filter((o) => !needle || o.label.toLowerCase().includes(needle) || o.sub?.toLowerCase().includes(needle)),
  )
</script>

<button
  bind:this={btnRef}
  type="button"
  onclick={() => {
    q = ''
    open = !open
  }}
  class={cn(chipSecondary, className)}
  {title}
>
  <Icon size={12} />
  <span class="max-w-28 truncate">{current?.label ?? value}</span>
</button>
{#if open && pos}
  <div
    use:portal
    bind:this={panelRef}
    in:pop={POPOVER}
    out:fade={QUICK}
    class={cn(popPanel, 'fixed z-[60] min-w-56 overflow-hidden')}
    style:left="{pos.left}px"
    style:top={pos.top !== undefined ? `${pos.top}px` : undefined}
    style:bottom={pos.bottom !== undefined ? `${pos.bottom}px` : undefined}
  >
    <PopSearch value={q} onChange={(v) => (q = v)} placeholder={`Search ${menuLabel.toLowerCase()}`} />
    <div class={popHeader}>{menuLabel}</div>
    {#each visible as o (o.value)}
      <button
        type="button"
        onclick={() => {
          onChange(o.value)
          open = false
        }}
        class={cn(popRow, o.value === value ? popRowSelected : 'text-muted')}
      >
        <span class="min-w-0 flex-1 truncate">{o.label}</span>
        {#if o.sub}
          <span class="max-w-44 shrink-0 truncate text-right font-mono text-[10px] tracking-[0.05em] text-ink-dim">
            {o.sub}
          </span>
        {/if}
      </button>
    {/each}
    {#if visible.length === 0}
      <div class="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>
    {/if}
  </div>
{/if}
