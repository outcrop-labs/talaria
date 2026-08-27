<script lang="ts">
  import { cn } from '@/lib/cn'
  import Popover from '@/components/ui/Popover.svelte'
  import MeterBars from '@/components/chat/MeterBars.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipPrimary, popHeader, popRow, popRowSelected } from '@/components/chat/chat-chrome'

  // The composer's model chip (Mercury spec §7): a 36px mono chip — ✳ glyph,
  // tier name, and a 3×12 meter showing where the pick sits on the agent's tier
  // ladder — that opens the §7 popover (search row with ⌘K hint, panel bg, mono
  // section header, hover fill, dashed-gold selected row). Beats the raw
  // <select> — keyboard-navigable and portaled so it escapes the composer's
  // clipping. The §7 shell (ui/Popover) owns the portal/outside-click/Esc
  // mechanics; `follow` keeps the panel glued to the chip on scroll, `up`
  // because the chip docks to the bottom of its surface.
  let { tiers, value, onChange }: { tiers: string[]; value: string; onChange: (t: string) => void } = $props()

  let open = $state(false)
  let q = $state('')
  const options = $derived(['', ...tiers]) // '' = the agent's main model

  // Fresh search on every open.
  $effect(() => {
    if (open) q = ''
  })

  const selectedIndex = $derived(Math.max(0, options.indexOf(value)))
  const label = $derived(value || 'main')
  const needle = $derived(q.trim().toLowerCase())
  const visible = $derived(options.filter((t) => (t || 'main model').toLowerCase().includes(needle)))
</script>

<Popover bind:open follow up offset={6} class="min-w-44 overflow-hidden">
  {#snippet trigger()}
    <button type="button" class={chipPrimary} title="Model tier for this chat">
      <span aria-hidden="true" class="text-[10px] leading-none">✳</span>
      <span class="max-w-24 truncate">{label}</span>
      <MeterBars total={options.length} lit={selectedIndex + 1} />
    </button>
  {/snippet}
  {#snippet content(close)}
    <PopSearch value={q} onChange={(v) => (q = v)} placeholder="Search tiers" />
    <div class={popHeader}>Model tier</div>
    {#each visible as t (t || 'main')}
      <button
        type="button"
        onclick={() => {
          onChange(t)
          close()
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
  {/snippet}
</Popover>
