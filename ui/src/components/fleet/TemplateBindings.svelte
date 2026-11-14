<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { useTemplates } from '@/lib/templates'

  // This agent's template overrides — they beat the board default wherever the
  // agent drafts or creates tickets, and shape its plan documents.
  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  const qc = useQueryClient()
  const autosave = useAutosave()
  // `{ data: templates = [] }` made a failed /api/templates look like "there
  // are no templates" — the Select offers only the fallback option, and the
  // read-only cells below render an em-dash, which says "nothing bound" about
  // an agent that may well have a binding.
  const list = listQuery(useTemplates(), { title: 'Could not load templates', variant: 'inline' })
  /** A bound template's name. One we cannot resolve is not "none": after a
   *  failed read it is the id, otherwise it is said to be missing. */
  const nameOf = (id: string | null) =>
    id === null ? null : (list.rows.find((t) => t.id === id)?.name ?? (list.failed ? id : 'A template that no longer exists'))
  const bind = async (patch: { ticketTemplateId?: string | null; planTemplateId?: string | null }) => {
    const res = await patchAgentMeta(def.id, patch)
    if (!res.error) autosave.flash('ticketTemplateId' in patch ? 'ticket' : 'plan')
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

{#snippet pick(kind: 'ticket' | 'plan', value: string | null, onChange: (v: string | null) => void)}
  {#if list.pending}
    <!-- Until templates land the select would show the fallback option and
         then flip: hold it with a select-shaped shimmer instead. -->
    <Skeleton class="h-8 w-[280px]" />
  {:else if canManage}
    <Select value={value ?? ''} size="sm" onchange={(e) => onChange(e.currentTarget.value || null)} class="w-[280px]">
      <option value="">{kind === 'ticket' ? 'Board default' : 'Freeform'}</option>
      {#each list.rows.filter((t) => t.kind === kind) as t (t.id)}
        <option value={t.id}>{t.name}</option>
      {/each}
    </Select>
  {:else}
    <span class="font-sans text-[13px] leading-5 text-fg">{nameOf(value) ?? (kind === 'ticket' ? 'Board default' : 'Freeform')}</span>
  {/if}
{/snippet}

<SettingsRow
  label="Ticket template"
  hint="Used wherever this agent writes tickets, ahead of the board's default."
  as={canManage ? 'label' : 'div'}
  saved={autosave.saved('ticket')}
>
  {@render pick('ticket', def.ticketTemplateId, (v) => void bind({ ticketTemplateId: v }))}
</SettingsRow>
<SettingsRow label="Plan template" hint="Shapes the plan documents this agent writes." as={canManage ? 'label' : 'div'} saved={autosave.saved('plan')}>
  {@render pick('plan', def.planTemplateId, (v) => void bind({ planTemplateId: v }))}
</SettingsRow>
{#if list.notice}<div class="px-5 py-3"><QueryError {...list.notice} /></div>{/if}
