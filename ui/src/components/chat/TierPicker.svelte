<script lang="ts">
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import MeterBars from '@/components/chat/MeterBars.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipPrimary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import { pop, POPOVER } from '@/lib/motion'

  // The composer's model chip (Mercury spec §7): a 36px mono chip — ✳ glyph,
  // tier name, and a 3×12 meter showing where the pick sits on the agent's tier
  // ladder — that opens the §7 popover (search row with ⌘K hint, panel bg, mono
  // section header, hover fill, dashed-gold selected row). Beats the raw
  // <select> — keyboard-navigable and portaled so it escapes the composer's
  // clipping.
  let { tiers, value, onChange }: { tiers: string[]; value: string; onChange: (t: string) => void } = $props()

  let open = $state(false)
  let q = $state('')
  let pos = $state<{ left: number; bottom: number } | null>(null)
  let btnRef = $state<HTMLButtonElement | null>(null)
  let panelRef = $state<HTMLDivElement | null>(null)
  const options = $derived(['', ...tiers]) // '' = the agent's main model

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

  const selectedIndex = $derived(Math.max(0, options.indexOf(value)))
  const label = $derived(value || 'main')
  const needle = $derived(q.trim().toLowerCase())
  const visible = $derived(options.filter((t) => (t || 'main model').toLowerCase().includes(needle)))
</script>

<button
  bind:this={btnRef}
  type="button"
  onclick={() => {
    q = ''
    open = !open
  }}
  class={chipPrimary}
  title="Model tier for this chat"
>
  <span aria-hidden="true" class="text-[10px] leading-none">✳</span>
  <span class="max-w-24 truncate">{label}</span>
  <MeterBars total={options.length} lit={selectedIndex + 1} />
</button>
{#if open && pos}
  <div
    use:portal
    bind:this={panelRef}
    in:pop={POPOVER}
    class={cn(popPanel, 'fixed z-[60] min-w-44 overflow-hidden')}
    style:left="{pos.left}px"
    style:bottom="{pos.bottom}px"
  >
    <PopSearch value={q} onChange={(v) => (q = v)} placeholder="Search tiers" />
    <div class={popHeader}>Model tier</div>
    {#each visible as t (t || 'main')}
      <button
        type="button"
        onclick={() => {
          onChange(t)
          open = false
        }}
        class={cn(popRow, t === value ? popRowSelected : 'text-muted')}
      >
        <span class="min-w-0 flex-1 truncate">{t || 'main model'}</span>
        <MeterBars total={options.length} lit={options.indexOf(t) + 1} class="shrink-0" />
      </button>
    {/each}
    {#if visible.length === 0}
      <div class="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>
    {/if}
  </div>
{/if}
