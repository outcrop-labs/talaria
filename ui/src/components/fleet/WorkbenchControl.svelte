<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Select from '@/components/ui/Select.svelte'
  import { getList } from '@/lib/fetch-json'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { slide } from '@/lib/motion'
  import { type WorkbenchProfileLite } from './workbench'
  import WorkbenchRepos from './WorkbenchRepos.svelte'
  import WorkbenchTuning from './WorkbenchTuning.svelte'

  // The one workbench setting. Auto shows what it resolves to for THIS agent
  // (fit by department/role); On lets an admin force a specific profile.
  let { def, isAdmin }: { def: AgentDef; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  // GET /api/workbench is 200 `{ profiles }` for any signed-in user — no 404 to
  // forgive. Answering `[]` on a failure made every agent read "nothing fits —
  // no sandbox", which is a claim an admin acts on (they go build a profile
  // that already exists).
  const profilesQuery = createQuery(() => ({
    queryKey: ['workbench-profiles'],
    queryFn: (): Promise<WorkbenchProfileLite[]> => getList<WorkbenchProfileLite>('/api/workbench', 'profiles'),
  }))
  const profiles = $derived(profilesQuery.data ?? [])
  const profilesFailed = $derived(profilesQuery.isError && profilesQuery.data === undefined)
  const mode = $derived(def.workbench ?? 'auto')
  const fits = (p: WorkbenchProfileLite) =>
    (p.autoAttach.departments ?? []).some((d) => d.toLowerCase() === def.department.toLowerCase()) ||
    (!!def.role && (p.autoAttach.roles ?? []).some((r) => (def.role ?? '').toLowerCase().includes(r.toLowerCase())))
  // Mirrors server resolveWorkbench (enabled profiles only) — keep in sync.
  const live = $derived(profiles.filter((p) => p.enabled))
  const resolved = $derived(
    mode === 'off'
      ? null
      : mode === 'on'
        ? (live.find((p) => p.slug === def.workbenchProfile) ?? live.find(fits) ?? live.find((p) => p.slug === 'dev') ?? null)
        : (live.find((p) => p.slug === def.workbenchProfile) ?? live.find(fits) ?? null),
  )

  const save = async (patch: Parameters<typeof patchAgentMeta>[1]) => {
    await patchAgentMeta(def.id, patch)
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

<div>
  <div class="mb-1 flex items-center gap-1.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Workbench</span>
    <InfoTip text="A sandboxed runtime scoped to this agent's role — tools, harnesses, and creds it can execute real work with. Auto attaches the profile that fits (department/role); On forces one; Off disables it." />
  </div>
  {#if isAdmin}
    <div class="flex flex-wrap items-center gap-2">
      <Segmented
        options={[
          { id: 'off', label: 'Off' },
          { id: 'auto', label: 'Auto' },
          { id: 'on', label: 'On' },
        ]}
        value={mode}
        onChange={(m) => void save({ workbench: m })}
      />
      {#if mode !== 'off' && profiles.length > 1}
        <Select
          size="sm"
          value={def.workbenchProfile ?? ''}
          onchange={(e) => void save({ workbenchProfile: e.currentTarget.value || null })}
        >
          <option value="">best fit</option>
          {#each profiles.filter((p) => p.enabled) as p (p.slug)}
            <option value={p.slug}>{p.name}</option>
          {/each}
        </Select>
      {/if}
      <span class="font-mono text-[11px] text-muted">
        {mode === 'off'
          ? 'no sandbox'
          : profilesFailed
            ? '→ unknown'
            : profilesQuery.data === undefined
              ? '→ checking'
              : resolved
                ? `→ ${resolved.name}${resolved.harnesses.length ? ` (${resolved.harnesses.join(', ')})` : ''}`
                : '→ nothing fits — no sandbox'}
      </span>
    </div>
  {:else}
    <div class="text-fg">
      {mode === 'off' ? 'Off' : profilesFailed ? 'Unknown' : profilesQuery.data === undefined ? 'Checking' : resolved ? resolved.name : 'None'}
    </div>
  {/if}
  <!-- "Nothing fits" and "we could not ask" are different answers. Only the
       first one means an admin should go build a profile. -->
  {#if mode !== 'off' && profilesFailed}
    <div transition:slide={{ duration: 150 }}>
      <QueryError
        variant="inline"
        class="mt-1.5"
        error={profilesQuery.error}
        title="Could not load workbench profiles"
        onRetry={() => void profilesQuery.refetch()}
      />
    </div>
  {/if}
  {#if isAdmin && mode !== 'off' && resolved}
    <div transition:slide={{ duration: 150 }}>
      <WorkbenchTuning {def} profile={resolved} {save} />
      <WorkbenchRepos agentId={def.id} />
    </div>
  {/if}
</div>
