<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { useTemplates } from '@/lib/templates'
  import Stat from './Stat.svelte'

  // This agent's template overrides — they beat the board default wherever the
  // agent drafts or creates tickets, and shape its plan documents.
  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  // `{ data: templates = [] }` made a failed /api/templates look like "there
  // are no templates" — the Select offers only the fallback option, and the
  // read-only cells below render an em-dash, which says "nothing bound" about
  // an agent that may well have a binding.
  const list = listQuery(useTemplates(), { title: 'Could not load templates', variant: 'inline' })
  const nameOf = (id: string | null) =>
    // A bound id we cannot resolve is not "none" — show the id, not an em-dash.
    id === null ? '—' : (list.rows.find((t) => t.id === id)?.name ?? (list.failed ? id : '—'))
  const bind = async (patch: { ticketTemplateId?: string | null; planTemplateId?: string | null }) => {
    await patchAgentMeta(def.id, patch)
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

{#snippet pick(kind: 'ticket' | 'plan', value: string | null, onChange: (v: string | null) => void)}
  <Select value={value ?? ''} size="sm" onchange={(e) => onChange(e.currentTarget.value || null)} class="w-full">
    <option value="">{kind === 'ticket' ? 'Board default' : 'Freeform'}</option>
    {#each list.rows.filter((t) => t.kind === kind) as t (t.id)}
      <option value={t.id}>{t.name}</option>
    {/each}
  </Select>
{/snippet}

<div>
  <div class="mb-1.5 flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    Templates
    <InfoTip text="A bound template wins over the board's default wherever this agent writes tickets or plans." />
  </div>
  {#if list.pending}
    <!-- Until templates land the selects would show the fallback option and
         then flip — hold both with select-shaped shimmer instead. -->
    <div class="grid grid-cols-2 gap-3">
      <div>
        <div class="mb-1 text-[11px] text-muted">Tickets</div>
        <Skeleton class="h-9 w-full" />
      </div>
      <div>
        <div class="mb-1 text-[11px] text-muted">Plan documents</div>
        <Skeleton class="h-9 w-full" />
      </div>
    </div>
  {:else if isAdmin}
    <div class="grid grid-cols-2 gap-3">
      <div>
        <div class="mb-1 text-[11px] text-muted">Tickets</div>
        {@render pick('ticket', def.ticketTemplateId, (v) => void bind({ ticketTemplateId: v }))}
      </div>
      <div>
        <div class="mb-1 text-[11px] text-muted">Plan documents</div>
        {@render pick('plan', def.planTemplateId, (v) => void bind({ planTemplateId: v }))}
      </div>
    </div>
  {:else}
    <div class="grid grid-cols-2 gap-3">
      <Stat label="Tickets" value={nameOf(def.ticketTemplateId)} />
      <Stat label="Plan documents" value={nameOf(def.planTemplateId)} />
    </div>
  {/if}
  {#if list.notice}<QueryError {...list.notice} />{/if}
</div>
