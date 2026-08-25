<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import { getJson } from '@/lib/fetch-json'
  import CategoryDetail from '@/components/models/CategoryDetail.svelte'
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
  const CATEGORIES: Array<{ id: string; label: string; blurb: string; ids: string[] }> = [
    {
      id: 'research',
      label: 'Research',
      blurb: 'The search stages behind every research run. Each stage needs a web-search-capable model; without live search it answers from memory and the citations come out invented.',
      ids: ['role:research-recon', 'role:research-brief', 'role:research-expedition'],
    },
    {
      id: 'workbench',
      label: 'Workbench',
      blurb: 'The effort ladder for coding work. Agents pick the effort; these settings pick the model that runs it.',
      ids: ['role:code-light', 'role:code-standard', 'role:code-heavy'],
    },
    {
      id: 'chores',
      label: 'Chores',
      blurb: 'The platform’s background upkeep: blurbs, titles, gists, digests. Everything here falls back to the Utility chain, so Utility is the one pick that moves them all.',
      ids: ['role:utility', 'agent:blurb-writer', 'agent:titler', 'agent:summarizer', 'agent:librarian'],
    },
    {
      id: 'writing',
      label: 'Writing',
      blurb: 'The Muse family: drafting and distillation that follows the requesting user’s muse, else the Utility chain.',
      ids: ['agent:muse', 'agent:distiller', 'agent:concluder', 'agent:briefer'],
    },
    {
      id: 'oversight',
      label: 'Oversight',
      blurb: 'Judging agents’ reported ticket outcomes against the ask: verdicts and findings on boards with judging on.',
      ids: ['agent:judge'],
    },
    {
      id: 'reserved',
      label: 'Reserved',
      blurb: 'Slots for surfaces that haven’t landed yet. Assign now; the pick takes effect the day the surface does.',
      ids: ['role:vision', 'role:image-generation', 'role:embedding', 'role:reranker'],
    },
  ]
  const categories = $derived.by(() => {
    const known = new Set(CATEGORIES.flatMap((c) => c.ids))
    const out = CATEGORIES.map(({ ids, ...c }) => ({ ...c, slots: slots.filter((s) => ids.includes(s.id)) })).filter(
      (c) => c.slots.length > 0,
    )
    const other = slots.filter((s) => !known.has(s.id))
    if (other.length)
      out.push({ id: 'other', label: 'Other', blurb: 'Settings that arrived after this view’s map was written.', slots: other })
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
    await fetch(slot.kind === 'role' ? '/api/admin/model-roles' : '/api/admin/platform-agents', {
      method: 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(slot.kind === 'role' ? { role: slot.row.role, ...patch } : { id: slot.row.id, ...patch }),
    })
    await qc.invalidateQueries({ queryKey: ['model-roles'] })
    await qc.invalidateQueries({ queryKey: ['platform-agents'] })
  }

  // The row's second line: the category's members' live states, in order —
  // the whole fleet still reads from the menu without clicking in.
  const sublineOf = (c: { slots: Slot[] }) => c.slots.map((s) => slotState(s, assignmentOf(s))).join(' · ')

  // Both reads green or neither has data — one load-failed state for the
  // pane, not two competing banners.
  const loadFailed = $derived((rolesQuery.isError && !rolesQuery.data) || (agentsQuery.isError && !agentsQuery.data))
</script>

<!-- Templates' pane height minus this view's ViewHeader row and its gap —
     the one structural difference between the two shells. -->
<div class="h-[calc(100vh-20.5rem)] min-h-[26rem]">
  <LibraryPane
    title="Roles & workers"
    groups={[{ items: categories }]}
    idOf={(c: (typeof categories)[number]) => c.id}
    labelOf={(c: (typeof categories)[number]) => c.label}
    selectedId={selected?.id ?? null}
    onSelect={(c: (typeof categories)[number]) => (selectedId = c.id)}
    pending={rolesQuery.isPending || agentsQuery.isPending}
    notice={loadFailed
      ? {
          error: rolesQuery.error ?? agentsQuery.error,
          title: 'Could not load the slot catalogs',
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
      <span class="block truncate">{c.label}</span>
      <!-- The members' live states as the row's second line — the fleet's
           state at a glance, which is what a library list is for. -->
      <span class="block truncate font-mono text-[10px] text-ink-dim">{sublineOf(c)}</span>
    {/snippet}

    {#snippet empty()}
      <EmptyState
        icon="▤"
        title="Nothing selected"
        hint={loadFailed ? 'The slot catalogs could not be loaded. Retry on the left.' : 'Pick a category on the left to set its models.'}
      />
    {/snippet}

    {#snippet detail()}
      {#if selected}
        {#key selected.id}
          <CategoryDetail
            label={selected.label}
            blurb={selected.blurb}
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
