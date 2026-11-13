<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import { getJson, putJson } from '@/lib/fetch-json'
  import { anyFailed, anyPending } from '@/components/ui/query-state'
  import { toastError } from '@/lib/toast.svelte'
  import CategoryDetail from '@/components/models/CategoryDetail.svelte'
  import { useModelNames } from '@/components/models/model-names.svelte'
  import { slotState, type ModelRoleRow, type PlatformAgentRow, type RoleIssue, type Slot } from '@/components/models/slot'

  // ── Roles & workers — which model runs what ────────────────────────────────
  // Two catalogs, one question. The old Platform tab answered it for
  // Talaria's own named workers in a stacked panel while this tab answered it
  // for activity classes in a library — two layouts for one act, so the
  // workers joined the library. Both PUT endpoints survive unchanged; this
  // pane only merges the ASK (and dispatches each save back to the route that
  // owns its validation).
  type ModelRolesData = {
    roles: ModelRoleRow[]
    assignments: Record<string, string>
    models: string[]
    issues: RoleIssue[]
    /** Per-role reasoning-effort preference (null = the model's own default). */
    efforts: Record<string, string | null>
  }
  type PlatformAgentsData = {
    agents: PlatformAgentRow[]
    assignments: Record<string, string>
    models: string[]
    /** Per-agent reasoning-effort preference (null = the model's default). */
    efforts: Record<string, string | null>
  }

  const qc = useQueryClient()
  const rolesQuery = createQuery(() => ({
    queryKey: ['model-roles'],
    queryFn: (): Promise<ModelRolesData> => getJson<ModelRolesData>('/api/admin/model-roles'),
  }))
  const agentsQuery = createQuery(() => ({
    queryKey: ['platform-agents'],
    queryFn: (): Promise<PlatformAgentsData> => getJson<PlatformAgentsData>('/api/admin/platform-agents'),
  }))

  // Both catalogs into one list of slots, each with a kind-prefixed id.
  const slots = $derived.by(() => {
    const out: Slot[] = []
    for (const r of rolesQuery.data?.roles ?? []) out.push({ kind: 'role', id: `role:${r.role}`, row: r })
    for (const a of agentsQuery.data?.agents ?? []) out.push({ kind: 'agent', id: `agent:${a.id}`, row: a })
    return out
  })
  const assignmentOf = (s: Slot) =>
    s.kind === 'role' ? rolesQuery.data?.assignments[s.row.role] : agentsQuery.data?.assignments[s.row.id]
  const effortOf = (s: Slot) =>
    (s.kind === 'role'
      ? rolesQuery.data?.efforts?.[s.row.role]
      : agentsQuery.data?.efforts?.[s.row.id]) ?? null

  // THE MENU IS THE CATEGORIES. Every setting in both catalogs files under
  // exactly one, and a category's members share the fallback chain its blurb
  // names — Chores rides the Utility chain (Utility at its head), Writing is
  // the Muse family, Oversight stands alone on pl-main — so the grouping
  // doubles as the resolution map. Anything a later build adds to either
  // catalog without a home here lands in Other rather than silently
  // vanishing from the menu.
  const CATEGORIES: Array<{ id: string; label: string; blurb: string; info?: string; ids: string[] }> = [
    {
      id: 'research',
      label: 'Research',
      blurb: 'The search stages behind every research run.',
      info: 'A stage fetches through a web-search tool the harness supplies, or through a model that browses on its own. Without either, a run has nothing to fetch with.',
      ids: ['role:research-recon', 'role:research-brief', 'role:research-expedition'],
    },
    {
      id: 'workbench',
      label: 'Coding',
      blurb: 'Agents pick how much effort a coding task gets. These pick the model that runs it.',
      ids: ['role:code-light', 'role:code-standard', 'role:code-heavy'],
    },
    {
      id: 'chores',
      label: 'Chores',
      blurb: 'Background upkeep: blurbs, titles, summaries and digests.',
      info: 'Everything here falls back to Utility, so Utility is the one pick that moves them all.',
      ids: ['role:utility', 'agent:blurb-writer', 'agent:titler', 'agent:summarizer', 'agent:librarian'],
    },
    {
      id: 'writing',
      label: 'Writing',
      blurb: 'Drafting and distilling. Each follows the model of the person asking, then Utility.',
      ids: ['agent:muse', 'agent:distiller', 'agent:concluder', 'agent:briefer'],
    },
    {
      id: 'oversight',
      label: 'Judging',
      blurb: 'Checks what agents report on a ticket against what was asked.',
      ids: ['agent:judge'],
    },
    {
      id: 'eyes',
      label: 'Vision',
      blurb: 'Reads images for agents whose own model cannot.',
      ids: ['role:vision'],
    },
    {
      id: 'reserved',
      label: 'Reserved',
      blurb: 'For features that have not shipped. A pick takes effect the day they do.',
      ids: ['role:image-generation', 'role:embedding', 'role:reranker'],
    },
  ]
  const categories = $derived.by(() => {
    const known = new Set(CATEGORIES.flatMap((c) => c.ids))
    const out = CATEGORIES.map(({ ids, ...c }) => ({ ...c, slots: slots.filter((s) => ids.includes(s.id)) })).filter(
      (c) => c.slots.length > 0,
    )
    const other = slots.filter((s) => !known.has(s.id))
    if (other.length)
      out.push({ id: 'other', label: 'Other', blurb: 'Settings added since this page was last arranged.', slots: other })
    return out
  })

  // A fixed menu, not a user library: the categories are known, so the view
  // opens ON one rather than on a nothing-selected zero state.
  let selectedId = $state<string | null>(null)
  $effect(() => {
    if (selectedId || categories.length === 0) return
    selectedId = categories[0]!.id
  })
  const selected = $derived(categories.find((c) => c.id === selectedId) ?? null)

  // One act, two endpoints: each route treats absent fields as "leave it
  // alone", so a model save and an effort save are the same PUT either way.
  // The refetch is what surfaces the unfit warning for a pick just made —
  // nothing blocks or reverts; the admin is told and the choice stands.
  const save = async (slot: Slot, patch: { model?: string | null; effort?: string | null }) => {
    try {
      await putJson<{ ok: true }>(
        slot.kind === 'role' ? '/api/admin/model-roles' : '/api/admin/platform-agents',
        slot.kind === 'role' ? { role: slot.row.role, ...patch } : { id: slot.row.id, ...patch },
      )
    } catch (e) {
      toastError('Could not save that setting', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['model-roles'] })
    await qc.invalidateQueries({ queryKey: ['platform-agents'] })
  }

  // The row's second line: the models the job's settings run on, each named
  // once, so the whole fleet still reads from the menu without clicking in.
  // A model the catalog names reads by that name, and one it does not is its id.
  const modelName = useModelNames()
  const sentence = (text: string) => text.charAt(0).toUpperCase() + text.slice(1)
  const sublineOf = (c: { slots: Slot[] }) => {
    const parts = new Map<string, { text: string; id: boolean }>()
    for (const s of c.slots) {
      const assigned = assignmentOf(s)
      const state = slotState(s, assigned)
      if (parts.has(state)) continue
      const name = state === assigned ? modelName(assigned) : sentence(state)
      parts.set(state, { text: name ?? state, id: !name })
    }
    return [...parts].map(([key, part]) => ({ key, ...part }))
  }

  // Both reads green or neither has data — one load-failed state for the
  // pane, not two competing banners.
  const loadFailed = $derived(anyFailed(rolesQuery, agentsQuery))
</script>

<!-- The window height left under the page header and the tab strip, so the
     pane ends where the page's bottom padding begins. -->
<div class="h-[calc(100vh-15.75rem)] min-h-[26rem]">
  <LibraryPane
    title="Jobs"
    groups={[{ items: categories }]}
    idOf={(c: (typeof categories)[number]) => c.id}
    labelOf={(c: (typeof categories)[number]) => c.label}
    selectedId={selected?.id ?? null}
    onSelect={(c: (typeof categories)[number]) => (selectedId = c.id)}
    pending={anyPending(rolesQuery, agentsQuery)}
    notice={loadFailed
      ? {
          error: rolesQuery.error ?? agentsQuery.error,
          title: 'Could not load the model jobs',
          onRetry: () => {
            void rolesQuery.refetch()
            void agentsQuery.refetch()
          },
          variant: 'compact',
        }
      : null}
    listWidth="w-72"
    class="h-full"
  >
    {#snippet row(c: (typeof categories)[number])}
      <span class="block truncate font-medium leading-5 text-fg">{c.label}</span>
      <span class="block truncate font-sans text-xs leading-4 text-muted">
        {#each sublineOf(c) as part, i (part.key)}{i > 0 ? ' · ' : ''}<span class={part.id ? 'font-mono' : undefined}>{part.text}</span>{/each}
      </span>
    {/snippet}

    {#snippet empty()}
      <EmptyState
        icon="▤"
        title="Nothing selected"
        hint={loadFailed ? 'The model jobs could not be loaded. Retry on the left.' : 'Pick a job on the left to set its models.'}
      />
    {/snippet}

    {#snippet detail()}
      {#if selected}
        {#key selected.id}
          <CategoryDetail
            label={selected.label}
            blurb={selected.blurb}
            info={selected.info}
            slots={selected.slots}
            models={rolesQuery.data?.models ?? agentsQuery.data?.models ?? []}
            issues={rolesQuery.data?.issues ?? []}
            {assignmentOf}
            {effortOf}
            onSave={(slot, patch) => void save(slot, patch)}
          />
        {/key}
      {/if}
    {/snippet}
  </LibraryPane>
</div>
