<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Chip from '@/components/ui/Chip.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { getJson } from '@/lib/fetch-json'

  // ── Platform sub-agents: Talaria's own workers, one model pick each ─────────
  interface PlatformAgentRow {
    id: string
    label: string
    job: string
    skills: string[]
    auto: string
    assignable: boolean
  }
  type PlatformAgentsData = { agents: PlatformAgentRow[]; assignments: Record<string, string>; models: string[] }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['platform-agents'],
    queryFn: (): Promise<PlatformAgentsData> => getJson<PlatformAgentsData>('/api/admin/platform-agents'),
  }))
  const assign = async (id: string, model: string | null) => {
    await fetch('/api/admin/platform-agents', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id, model }),
    })
    await qc.invalidateQueries({ queryKey: ['platform-agents'] })
  }
</script>

{#if !query.data}
  <!-- A failed read used to shimmer for ever — the skeleton was the only
       not-loaded branch, so a 500 looked like a slow network. -->
  <Panel>
    {#if query.isError}
      <QueryError
        variant="compact"
        error={query.error}
        title="Could not load platform agents"
        onRetry={() => void query.refetch()}
      />
    {:else}
      <Skeleton class="mb-4 h-4 w-32 rounded-full" />
      <div class="space-y-4">
        {#each Array.from({ length: 6 }, (_, i) => i) as i (i)}
          <div class="flex items-center gap-3">
            <div class="min-w-0 flex-1 space-y-1.5">
              <Skeleton class="h-3 w-36 rounded-full" delay={i * 0.1} />
              <Skeleton class="h-2.5 w-72 rounded-full" delay={i * 0.1 + 0.06} />
            </div>
            <Skeleton class="h-8 w-56 shrink-0" delay={i * 0.1} />
          </div>
        {/each}
      </div>
    {/if}
  </Panel>
{:else}
  {@const data = query.data}
  <Panel>
    <SectionHeader
      title="Platform agents"
      info="Talaria's own sub-agents — the workers behind internal jobs like distilling chats and drafting with Muse. Separate from your Hermes fleet: each has its own harness and skills, and you pick its model here. Unset = auto (each job's own sensible chain)."
    />
    <ul class="divide-y divide-line">
      {#each data.agents as a (a.id)}
        <li class="flex items-center gap-3 py-3">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2 font-sans text-sm text-fg">
              {a.label}
              {#each a.skills as sk (sk)}
                <Chip>{sk}</Chip>
              {/each}
            </div>
            <div class="font-sans text-xs text-muted">{a.job}</div>
            <div class="font-mono text-[11px] text-muted/80">Auto: {a.auto}</div>
          </div>
          {#if a.assignable}
            <Select
              size="sm"
              class="w-56 shrink-0"
              value={data.assignments[a.id] ?? ''}
              onchange={(e) => void assign(a.id, e.currentTarget.value || null)}
            >
              <option value="">Auto</option>
              {#each data.models as m (m)}
                <option value={m}>
                  {m}
                </option>
              {/each}
            </Select>
          {:else}
            <span class="w-56 shrink-0 text-right font-mono text-[11px] text-muted" title={a.auto}>
              fixed by design
            </span>
          {/if}
        </li>
      {/each}
    </ul>
  </Panel>
{/if}
