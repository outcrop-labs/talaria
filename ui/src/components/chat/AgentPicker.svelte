<script lang="ts">
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { focusGold, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import type { AgentModel } from '@/lib/agents'

  // The agent switcher — pick which fleet agent you're talking to.
  let {
    agents,
    value,
    onChange,
    loading,
    fullWidth,
  }: {
    agents: AgentModel[]
    value: string | null
    onChange: (id: string) => void
    loading?: boolean
    fullWidth?: boolean
  } = $props()

  let open = $state(false)
  let q = $state('')
  let ref = $state<HTMLDivElement | null>(null)
  const current = $derived(agents.find((a) => a.id === value) ?? null)

  $effect(() => {
    if (!open) return
    const onDocClick = (e: MouseEvent) => {
      if (ref && !ref.contains(e.target as Node)) open = false
    }
    document.addEventListener('mousedown', onDocClick)
    return () => document.removeEventListener('mousedown', onDocClick)
  })

  const needle = $derived(q.trim().toLowerCase())
  const visible = $derived(
    agents.filter((a) => !needle || a.label.toLowerCase().includes(needle) || a.role?.toLowerCase().includes(needle)),
  )
</script>

<div bind:this={ref} class={cn('relative', fullWidth && 'w-full')}>
  <!-- Select-shaped trigger (spec §8): raised tile, hairline border, radius 6. -->
  <button
    type="button"
    disabled={loading || agents.length === 0}
    onclick={() => {
      q = ''
      open = !open
    }}
    class={cn(
      'flex items-center gap-2 rounded-md border border-line bg-raised font-sans text-sm transition-colors hover:border-line-strong disabled:opacity-60',
      focusGold,
      fullWidth ? 'w-full px-2 py-1.5' : 'py-1 pl-1 pr-3',
    )}
  >
    {#if loading}
      <!-- Picker-shaped shimmer while the fleet loads (button stays disabled). -->
      <Skeleton class="h-7 w-7 shrink-0 rounded-full" />
      <span class="min-w-0 flex-1 space-y-1.5 text-left">
        <Skeleton class="h-2.5 w-24 rounded-full" delay={0.12} />
        <Skeleton class="h-2 w-16 rounded-full" delay={0.24} />
      </span>
    {:else}
      <Avatar name={current?.label} />
      <span class="min-w-0 flex-1 text-left">
        <span class="block truncate text-fg">{current ? current.label : 'Select an agent'}</span>
        {#if current?.role}<span class="block truncate text-xs text-muted">{current.role}</span>{/if}
      </span>
    {/if}
    <span class="text-muted">▾</span>
  </button>

  {#if open}
    <!-- Entrance comes from popPanel's gd-enter (the §9 popover motif) — the
        old fly on top of it double-animated. Exit gains the grammar fade. -->
    <div out:fade={QUICK} class={cn(popPanel, 'absolute left-0 z-20 mt-2', fullWidth ? 'w-full' : 'w-64')}>
      <!-- §7 popover pattern: search field on top (mono placeholder, ⌘K hint). -->
      <PopSearch value={q} onChange={(v) => (q = v)} placeholder="Search agents" />
      <ul class="max-h-72 overflow-auto">
        {#each visible as a (a.id)}
          <li>
            <button
              type="button"
              onclick={() => {
                onChange(a.id)
                open = false
              }}
              class={cn(popRow, 'py-2', a.id === value ? popRowSelected : 'text-muted')}
            >
              <Avatar name={a.label} />
              <span class="min-w-0 flex-1">
                <span class="block truncate text-fg">{a.label}</span>
                {#if a.role}
                  <span class="block truncate font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
                    {a.role}
                  </span>
                {/if}
              </span>
              {#if a.id === value}<span aria-hidden="true" class="h-[7px] w-[7px] shrink-0 rounded-full bg-accent"></span>{/if}
            </button>
          </li>
        {/each}
        {#if visible.length === 0}<li class="px-2 py-1.5 font-sans text-[13px] text-muted">No matches</li>{/if}
      </ul>
    </div>
  {/if}
</div>
