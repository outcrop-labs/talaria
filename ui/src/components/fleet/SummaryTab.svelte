<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Input from '@/components/ui/Input.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { useFleet, relativeTime } from '@/lib/fleet'
  import { patchAgentMeta, type AgentDef, type ModelTarget } from '@/lib/fleet-defs'
  import Stat from './Stat.svelte'
  import TemplateBindings from './TemplateBindings.svelte'
  import WorkbenchControl from './WorkbenchControl.svelte'

  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const cfg = $derived(def.latest?.config)
  const fleetQuery = useFleet()
  const stat = $derived(fleetQuery.data?.agents.find((a) => a.id === def.model))
  let role = $state(def.role ?? '')
  const saveRole = async () => {
    if (role.trim() === (def.role ?? '')) return
    await patchAgentMeta(def.id, { role: role.trim() || null })
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

<!-- Model identity is chrome — mono chip, radius 6, class in signal color. -->
{#snippet targetChip(t: ModelTarget, name?: string)}
  {@const local = /inference|vllm|ollama|spark|local/.test(t.endpoint)}
  <span class="inline-flex items-center gap-1.5 rounded-md border border-line px-2 py-0.5 font-mono text-[10px] tracking-[0.05em]">
    {#if name}<span class="font-medium uppercase text-fg">{name}</span>{/if}
    <span class="text-muted">{t.model}</span>
    <span class={cn('uppercase', local ? 'text-success' : 'text-accent')}>{local ? 'self-hosted' : 'cloud'}</span>
  </span>
{/snippet}

<!-- A Stat cell's shape while its query is in flight: label bar + value bar. -->
{#snippet statSkeleton(delay: number = 0)}
  <div class="space-y-1.5">
    <Skeleton class="h-2.5 w-20 rounded-full" {delay} />
    <Skeleton class="h-3.5 w-24 rounded-full" delay={delay + 0.12} />
  </div>
{/snippet}

<div class="space-y-4 text-sm">
  <!-- Editable role — the human-readable title shown on the roster. -->
  <div>
    <div class="mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Role</div>
    {#if isAdmin}
      <Input size="sm" bind:value={role} onblur={() => void saveRole()} placeholder="e.g. Support Lead" class="max-w-xs" />
    {:else}
      <div class="text-fg">{def.role ?? '—'}</div>
    {/if}
  </div>
  <!-- Workbench — THE sandbox setting: off / auto (fit rules) / on. -->
  <WorkbenchControl {def} {isAdmin} />
  <div class="grid grid-cols-2 gap-3">
    <Stat label="Model id" value={def.model} />
    <Stat label="Department" value={def.department} />
    <Stat label="Management" value="Talaria-managed" />
    <Stat label="Version" value={`v${def.currentVersion}`} />
    {#if fleetQuery.isLoading}
      <!-- The usage cells land late and grow the grid — hold their spots. -->
      {@render statSkeleton()}
      {@render statSkeleton(0.12)}
      {@render statSkeleton(0.24)}
    {:else}
      {#if stat}<Stat label="Conversations" value={String(stat.conversations)} />{/if}
      {#if stat}<Stat label="Messages" value={String(stat.messages)} />{/if}
      {#if stat?.lastUsed}<Stat label="Last used" value={relativeTime(stat.lastUsed)} />{/if}
    {/if}
  </div>
  <div>
    <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Models</div>
    <div class="flex flex-wrap items-center gap-2">
      {#if cfg?.main}{@render targetChip(cfg.main, 'main')}{/if}
      {#each cfg?.aliases ?? [] as a (a.name)}{@render targetChip(a, a.name)}{/each}
      {#if !cfg?.main}<span class="text-xs text-muted">no config yet</span>{/if}
    </div>
    {#if cfg?.fallbacks?.length}
      <div class="mt-2 text-xs text-muted">↯ fallback: {cfg.fallbacks.map((f) => f.model).join(' → ')}</div>
    {/if}
  </div>
  {#if cfg?.mcpServers?.length}
    <Stat label="MCP" value={cfg.mcpServers.join(', ')} />
  {/if}
  <TemplateBindings {def} {isAdmin} />
</div>
