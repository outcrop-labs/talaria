<script lang="ts">
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import MeterBars from '@/components/chat/MeterBars.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipPrimary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'

  /** The composer rail's agent chip (spec §7): assistant-chip style — strong border,
   *  mono uppercase agent name, 3×12 meter marking where the pick sits in the
   *  fleet (capped at the spec's five bars) — opening the §7 popover (search
   *  row with ⌘K hint, mono header, right-aligned mono role meta, dashed-gold
   *  selected row). A conversation is bound to its agent, so picking here hands
   *  off to the host's route-level agent selection (same behavior as picking
   *  the agent in the sidebar — the host swaps to that agent's thread). */
  let {
    agents,
    value,
    onChange,
    class: className,
  }: {
    agents: { id: string; label: string; role?: string }[]
    value: string
    onChange: (id: string) => void
    class?: string
  } = $props()

  let open = $state(false)
  let q = $state('')
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

  const current = $derived(agents.find((a) => a.id === value))
  const total = $derived(Math.min(agents.length, 5))
  const meterLit = (id: string) => {
    const i = Math.max(0, agents.findIndex((a) => a.id === id))
    return Math.max(1, Math.round(((i + 1) / Math.max(1, agents.length)) * total))
  }
  const needle = $derived(q.trim().toLowerCase())
  const visible = $derived(
    agents.filter((a) => !needle || a.label.toLowerCase().includes(needle) || a.role?.toLowerCase().includes(needle)),
  )
</script>

<button
  bind:this={btnRef}
  type="button"
  onclick={() => {
    q = ''
    open = !open
  }}
  class={cn(chipPrimary, className)}
  title="Agent for this conversation"
>
  <span class="max-w-28 truncate">{current?.label ?? value}</span>
  <MeterBars {total} lit={meterLit(value)} />
</button>
{#if open && pos}
  <div
    use:portal
    bind:this={panelRef}
    class={cn(popPanel, 'fixed z-[60] min-w-56 overflow-hidden')}
    style:left="{pos.left}px"
    style:bottom="{pos.bottom}px"
  >
    <PopSearch value={q} onChange={(v) => (q = v)} placeholder="Search agents" />
    <div class={popHeader}>Agent</div>
    <div class="max-h-72 overflow-auto">
      {#each visible as a (a.id)}
        <button
          type="button"
          onclick={() => {
            open = false
            if (a.id !== value) onChange(a.id)
          }}
          class={cn(popRow, a.id === value ? popRowSelected : 'text-muted')}
        >
          <span class="min-w-0 flex-1 truncate">{a.label}</span>
          {#if a.role}
            <span class="max-w-40 shrink-0 truncate text-right font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
              {a.role}
            </span>
          {/if}
          <MeterBars {total} lit={meterLit(a.id)} class="shrink-0" />
        </button>
      {/each}
      {#if visible.length === 0}
        <div class="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</div>
      {/if}
    </div>
  </div>
{/if}
