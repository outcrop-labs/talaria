<script lang="ts">
  import { PlugZap, Sparkles } from '@lucide/svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipSecondary, popHeader, popPanel, popRow } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  // Aliased: the local `pop` popover controller shadows motion's `pop`.
  import { listStagger, pop as popIn, POPOVER } from '@/lib/motion'
  import type { RailItem } from './assistant-composer-controls'
  import { createAnchoredPopover } from './anchored-popover.svelte'

  let { kind, items }: { kind: 'mcp' | 'skills'; items: RailItem[] } = $props()

  let open = $state(false)
  let query = $state('')
  const pop = createAnchoredPopover(
    () => open,
    (next) => (open = next),
  )
  const visible = $derived.by(() => {
    const needle = query.trim().toLowerCase()
    return items.filter((item) => !needle || item.label.toLowerCase().includes(needle) || item.detail?.toLowerCase().includes(needle))
  })
  const width = $derived(kind === 'mcp' ? 'w-[76px]' : 'w-[70px]')
</script>

<button
  bind:this={pop.button}
  type="button"
  onclick={() => {
    query = ''
    open = !open
  }}
  class={cn(chipSecondary, width, 'justify-center gap-1.5')}
  title={kind === 'mcp' ? 'MCP access for the selected agent' : 'Skills available to the selected agent'}
>
  {#if kind === 'mcp'}<span class="h-1.5 w-1.5 rounded-full bg-accent" aria-hidden="true"></span>{/if}
  {#if kind === 'skills'}<Sparkles size={11} aria-hidden="true" />{/if}
  <span>{kind} {items.length}</span>
</button>
{#if open && pop.position}
  <div
    use:portal
    bind:this={pop.panel}
    in:popIn={POPOVER}
    class={cn(popPanel, 'fixed z-[70] w-64 overflow-hidden')}
    style:left="{pop.position.left}px"
    style:bottom="{pop.position.bottom}px"
  >
    <PopSearch value={query} onChange={(v) => (query = v)} placeholder={kind === 'mcp' ? 'Search MCPs' : 'Search skills'} />
    <div class={popHeader}>{kind === 'mcp' ? 'Agent MCP access' : 'Available skills'}</div>
    <div class="max-h-64 overflow-y-auto" use:listStagger>
      {#each visible as item (item.id)}
        <div class={cn(popRow, 'text-muted')}>
          {#if kind === 'mcp'}<PlugZap size={12} class="shrink-0 text-accent" />{:else}<Sparkles size={12} class="shrink-0 text-accent" />{/if}
          <span class="min-w-0 flex-1">
            <span class="block truncate text-fg">{item.label}</span>
            {#if item.detail}<span class="block truncate font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">{item.detail}</span>{/if}
          </span>
        </div>
      {/each}
      {#if visible.length === 0}<div class="px-2 py-2 font-sans text-[13px] text-muted">None available</div>{/if}
    </div>
  </div>
{/if}
