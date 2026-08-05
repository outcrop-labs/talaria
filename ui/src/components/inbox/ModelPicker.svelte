<script lang="ts">
  import MeterBars from '@/components/chat/MeterBars.svelte'
  import PopSearch from '@/components/chat/PopSearch.svelte'
  import { chipPrimary, popHeader, popPanel, popRow, popRowSelected } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { portal } from '@/lib/portal'
  import type { GatewayModel } from '@/lib/muse.svelte'
  import { createAnchoredPopover } from './anchored-popover.svelte'

  let {
    models,
    value,
    onChange,
    loading,
  }: {
    models: GatewayModel[]
    value: string
    onChange: (model: string) => void
    loading: boolean
  } = $props()

  let open = $state(false)
  let query = $state('')
  const pop = createAnchoredPopover(
    () => open,
    (next) => (open = next),
  )
  const current = $derived(models.find((model) => model.id === value))
  const visible = $derived(
    models.filter((model) => {
      const needle = query.trim().toLowerCase()
      return !needle || model.id.toLowerCase().includes(needle) || model.label?.toLowerCase().includes(needle)
    }),
  )
  const selectedIndex = $derived(Math.max(0, models.findIndex((model) => model.id === value)))

  function modelLabel(model: GatewayModel | undefined, fallback: string): string {
    const label = model?.label?.trim() || model?.id || fallback
    return label.replace(/^claude[- ]?/i, '').replace(/^anthropic[/: -]*/i, '')
  }
</script>

<button
  bind:this={pop.button}
  type="button"
  disabled={loading || models.length === 0}
  onclick={() => {
    query = ''
    open = !open
  }}
  class={cn(chipPrimary, 'w-[108px] justify-between disabled:opacity-40')}
  title="Model for the assistant's response"
>
  <span aria-hidden="true" class="text-accent">✦</span>
  <span class="min-w-0 flex-1 truncate text-left">{loading ? 'Loading' : modelLabel(current, 'Model')}</span>
  <MeterBars total={3} lit={Math.max(1, Math.ceil(((selectedIndex + 1) / Math.max(1, models.length)) * 3))} />
</button>
{#if open && pop.position}
  <div
    use:portal
    bind:this={pop.panel}
    class={cn(popPanel, 'fixed z-[70] w-72 overflow-hidden')}
    style:left="{pop.position.left}px"
    style:bottom="{pop.position.bottom}px"
  >
    <PopSearch value={query} onChange={(v) => (query = v)} placeholder="Search models" />
    <div class={popHeader}>Available models</div>
    <div class="max-h-72 overflow-y-auto">
      {#each visible as model (model.id)}
        <button
          type="button"
          onclick={() => {
            onChange(model.id)
            open = false
          }}
          class={cn(popRow, model.id === value ? popRowSelected : 'text-muted')}
        >
          <span aria-hidden="true" class="text-accent">✦</span>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-fg">{model.label || model.id}</span>
            {#if model.label}<span class="block truncate font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">{model.id}</span>{/if}
          </span>
        </button>
      {/each}
      {#if visible.length === 0}<div class="px-2 py-2 font-sans text-[13px] text-muted">No models found</div>{/if}
    </div>
  </div>
{/if}
