<script lang="ts">
  import { Gauge } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import { fade, pop, POPOVER, QUICK } from '@/lib/motion'
  import { chipSecondary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'

  // The composer's reasoning-effort chip (Mercury spec §7): a secondary 36px
  // mono chip — gauge glyph + current level — that opens the §7 popover (panel
  // bg, mono section header, hover fill, dashed-gold selected row). Rendered
  // by the chat surfaces ONLY with a non-empty `efforts` list, which the
  // server derives from the model's own catalog metadata: a model that
  // publishes no levels gets no picker and its requests carry no effort. ''
  // (the default row) omits the parameter entirely — the model's own default
  // effort, which is a real setting and not "none".
  let {
    efforts,
    value,
    onChange,
    disabled = false,
  }: {
    /** The levels this model supports, per its catalog metadata — the picker's
     *  whole option list; nothing else is offered. */
    efforts: string[]
    /** '' = the model's default (no parameter sent). */
    value: string
    onChange: (v: string) => void
    disabled?: boolean
  } = $props()

  // What each well-known level reads as, right-aligned in the row. Providers
  // coin levels this map has never heard of; those rows simply carry no sub
  // rather than a guess.
  const SUBS: Record<string, string> = {
    none: 'no reasoning',
    minimal: 'fastest',
    low: 'fast',
    medium: 'balanced',
    high: 'thorough',
    xhigh: 'deeper',
    max: 'deepest',
  }

  let open = $state(false)
  let pos = $state<{ left: number; bottom: number } | null>(null)
  let btnRef = $state<HTMLButtonElement | null>(null)
  let panelRef = $state<HTMLDivElement | null>(null)

  $effect(() => {
    if (!open) return
    const place = () => {
      const r = btnRef?.getBoundingClientRect()
      if (r) pos = { left: r.left, bottom: window.innerHeight - r.top + 6 }
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

  // The chip's readout: the picked level, or 'auto' for the model default.
  const label = $derived(value || 'auto')
</script>

<button
  bind:this={btnRef}
  type="button"
  onclick={() => (open = !open)}
  disabled={disabled}
  class={cn(chipSecondary, 'shrink-0')}
  title="Reasoning effort for this reply"
  aria-haspopup="listbox"
  aria-expanded={open}
>
  <Gauge size={12} />
  <span class="max-w-24 truncate">{label}</span>
</button>
{#if open && pos}
  <div
    use:portal
    bind:this={panelRef}
    in:pop={POPOVER}
    out:fade={QUICK}
    class={cn(popPanel, 'fixed z-[60] min-w-48 overflow-hidden')}
    style:left="{pos.left}px"
    style:bottom="{pos.bottom}px"
  >
    <div class={popHeader}>Reasoning effort</div>
    <button
      type="button"
      onclick={() => {
        onChange('')
        open = false
      }}
      class={cn(popRow, value === '' ? popRowSelected : 'text-muted')}
    >
      <span class="min-w-0 flex-1 truncate">auto</span>
      <span class="shrink-0 text-right font-mono text-[10px] tracking-[0.05em] text-ink-dim">model default</span>
    </button>
    {#each efforts as level (level)}
      <button
        type="button"
        onclick={() => {
          onChange(level)
          open = false
        }}
        class={cn(popRow, level === value ? popRowSelected : 'text-muted')}
      >
        <span class="min-w-0 flex-1 truncate">{level}</span>
        {#if SUBS[level]}
          <span class="shrink-0 text-right font-mono text-[10px] tracking-[0.05em] text-ink-dim">{SUBS[level]}</span>
        {/if}
      </button>
    {/each}
  </div>
{/if}
