<script lang="ts">
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Plus, Lock } from '@lucide/svelte'
  import { navigate } from '@/router'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import ViewHeader from '@/components/ui/ViewHeader.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import WorkflowDetail from '@/components/workflows/WorkflowDetail.svelte'
  import SkillEditor from '@/components/skills/SkillEditor.svelte'
  import StudioGuide, { type GuidePrefill } from '@/components/workflows/StudioGuide.svelte'
  import { cn } from '@/lib/cn'
  import { staggerIn } from '@/lib/motion'
  import { useAgents } from '@/lib/agents'
  import { useSession } from '@/lib/session'
  import { useBoards } from '@/lib/boards.svelte'
  import {
    deleteWorkflow,
    setGapStatus,
    useGaps,
    useSkillLibrary,
    useWorkflows,
    type CapabilityGap,
    type SkillLibraryOwner,
    type TaskWorkflow,
  } from '@/lib/workflows'
  import SectionTitle from './studio/SectionTitle.svelte'
  import SkillRow from './studio/SkillRow.svelte'

  // The Studio — build your agents, one at a time. The rail picks who you're
  // building for ("Every agent" = shared know-how, or one agent); the dashboard
  // is that agent's whole working world:
  //   Suggestions   what IT asked for (capability gaps, ranked by recurrence)
  //   Knows         its skills — own + inherited shared know-how
  //   Routed here   the workflows that steer ticket work onto it
  // "Teach" runs the guided flow (name → recognize → Muse-drafted skill → done)
  // with this agent already chosen. Skills/workflows stay Hermes-native
  // underneath — this surface only decides who learns what.

  // /studio?a=<owner> · &sk=<owner>/<name> opens a skill · &w=<id> a workflow
  const sp = (k: string): string | undefined => {
    const v = searchParams.get(k)
    return v == null || v === '' ? undefined : String(v)
  }

  const qc = useQueryClient()
  // The skill library IS this page's spine: `owners` drives the rail, the
  // selection, and the "who am I building for" pane. Dropping its rejection
  // made a DOWN SKILLS SERVICE render "No agents yet" — a sentence about the
  // FLEET, from a read that never asked about the fleet.
  const libraryQuery = useSkillLibrary()
  const rawOwners = $derived(libraryQuery.data ?? [])
  const isLoading = $derived(libraryQuery.isLoading)
  // PLATFORM skills (talaria-toolkit and friends) are plumbing, not
  // teachable know-how — the Studio doesn't show them at all.
  const owners = $derived(rawOwners.map((o) => ({ ...o, skills: o.skills.filter((sk) => !sk.platform) })))
  // The four side reads that DECORATE this page. Each was destructured with a
  // `= []` (or read through `?.`), which is why /api/agents at 500 rendered
  // byte-identically to a 200 returning nothing: "Routed here" empty means
  // "no workflows steer work here", a missing Suggestions section means "it
  // hasn't asked for anything", and neither is true when the fetch broke.
  // Collected once, listed once — no per-site guard to forget next time.
  const workflowsQuery = useWorkflows()
  const gapsQuery = useGaps()
  const agentsQuery = useAgents()
  const boardsQuery = useBoards()
  const workflows = $derived(workflowsQuery.data ?? [])
  const gaps = $derived(gapsQuery.data ?? [])
  const agentsData = $derived(agentsQuery.data)
  const boards = $derived(boardsQuery.data ?? [])
  const sideReads: Array<{ key: string; title: string; query: { isError: boolean; error: unknown; refetch: () => unknown } }> = [
    { key: 'workflows', title: 'Could not load workflows — “Routed here” is incomplete', query: workflowsQuery },
    { key: 'gaps', title: 'Could not load suggestions — what the agents asked for is missing', query: gapsQuery },
    { key: 'agents', title: 'Could not load the fleet — agent roles fall back to model ids', query: agentsQuery },
    { key: 'boards', title: 'Could not load boards — workflow rules name board ids instead', query: boardsQuery },
  ]
  const sideFailures = $derived(sideReads.filter((r) => r.query.isError))
  const sessionQuery = useSession()
  const isAdmin = $derived(sessionQuery.data?.role === 'admin')
  let guide = $state<(GuidePrefill & { gapId?: string }) | null>(null)

  const agents = $derived(owners.filter((o) => o.owner !== 'shared'))
  const shared = $derived(owners.find((o) => o.owner === 'shared'))
  const selectedKey = $derived(sp('a') ?? agents.find((o) => o.canEdit)?.owner ?? agents[0]?.owner ?? 'shared')
  const selected = $derived(owners.find((o) => o.owner === selectedKey) ?? shared ?? null)
  const select = (a: string) => void navigate('/studio', { search: { a } })

  const sharedSkills = $derived(new Set(shared?.skills.map((s) => s.name) ?? []))
  const boardName = (id: string) => boards.find((b) => b.id === id)?.name ?? 'a board'
  const agentMeta = (o: SkillLibraryOwner) => agentsData?.agents.find((a) => a.id === o.model)
  const firstName = (o: SkillLibraryOwner) => o.label.split(' ')[0]!

  /** Workflows steering work onto this owner: any bound skill it carries.
   *  The shared view also collects workflows whose skills nobody carries. */
  const routedTo = (o: SkillLibraryOwner): TaskWorkflow[] => {
    const own = new Set(o.skills.map((s) => s.name))
    const carriedBySomeone = new Set(owners.flatMap((x) => x.skills.map((s) => s.name)))
    return workflows.filter((w) => {
      if (o.owner === 'shared') return w.skills.length === 0 || w.skills.some((sk) => sharedSkills.has(sk) || !carriedBySomeone.has(sk))
      return w.skills.some((sk) => own.has(sk) || sharedSkills.has(sk))
    })
  }

  /** Plain-English row for a workflow — what it does, not how it's stored. */
  const describe = (w: TaskWorkflow): string => {
    const where = [
      w.match.boards?.length ? `on ${w.match.boards.map(boardName).join(' or ')}` : '',
      w.match.labels?.length ? `labeled ${w.match.labels.map((l) => `“${l}”`).join(' or ')}` : '',
      w.match.keywords?.length ? `mentioning ${w.match.keywords.map((k) => `“${k}”`).join(' or ')}` : '',
    ]
      .filter(Boolean)
      .join(', ')
    const flow = w.skills.length ? `follow ${w.skills.map((sk) => `“${sk}”`).join(' + ')}` : 'no skills bound yet'
    return `Tickets ${where || '(no rules yet)'} → ${flow}`
  }

  const ownerGaps = (o: SkillLibraryOwner): CapabilityGap[] =>
    o.owner === 'shared' ? gaps : gaps.filter((g) => g.agentModel === o.model)

  // Overlays driven by URL params
  const skOwner = $derived(sp('sk')?.split('/', 2)[0])
  const skName = $derived(sp('sk')?.split('/', 2)[1])
  const skOwnerInfo = $derived(owners.find((o) => o.owner === skOwner))
  const openWorkflow = $derived(workflows.find((w) => w.id === sp('w')) ?? null)
  const closeOverlay = () => void navigate('/studio', { search: { a: selectedKey } })

  const removeWorkflow = async (w: TaskWorkflow) => {
    if (!(await confirm({ title: 'Delete workflow', message: `Delete "${w.name}"? Matching tickets dispatch without its skills.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteWorkflow(w.id)
    closeOverlay()
    await qc.invalidateQueries({ queryKey: ['workflows'] })
  }
</script>

{#if isLoading}
  <div class="h-full overflow-y-auto p-8">
    <div class="mx-auto max-w-6xl">
      <SkeletonRows rows={8} avatar />
    </div>
  </div>
{:else}
  <div class="h-full overflow-y-auto p-8">
    <!-- Page content entrance AND post-skeleton reveal in one: this branch
         mounts when the library lands, so title → failures → rail+world grid
         rise in sequence (ANIMATIONS.md). Never on the skeleton branch. -->
    <div use:staggerIn class="mx-auto max-w-6xl">
      <ViewHeader
        class="mb-6"
        title="Studio"
        info="Build your agents, one at a time: what each one knows (skills), what work gets routed to it (workflows), and what it's asked for help with. Pick who you're building for on the left."
      />

      {#if sideFailures.length > 0}
        <div class="mb-6 space-y-2 rounded-xl border border-line-subtle p-3">
          {#each sideFailures as r (r.key)}
            <QueryError
              variant="inline"
              title={r.title}
              error={r.query.error}
              onRetry={() => void r.query.refetch()}
            />
          {/each}
        </div>
      {/if}

      <!-- The rail+world grid is the section whose items cascade: the frame
           fades while the rail's rows and the world's blocks (agent header,
           Suggestions, Knows, Routed here) rise 30ms apart, capped —
           staggerIn's data-stagger-items contract. -->
      <div class="grid gap-8 lg:grid-cols-[15rem_minmax(0,1fr)]" data-stagger-items="aside > *, main > *">
        <!-- ── Who you're building for ── -->
        <aside class="space-y-1">
          <div class="mb-2 px-2.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Building for</div>
          {#each agents as o (o.owner)}
            {@const meta = agentMeta(o)}
            {@const openCount = ownerGaps(o).length}
            <button
              type="button"
              onclick={() => select(o.owner)}
              class={cn(
                'flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left transition-colors',
                selectedKey === o.owner ? 'bg-raised' : 'hover:bg-hover',
              )}
            >
              <Avatar name={o.label} class="h-8 w-8 shrink-0 text-xs" />
              <span class="min-w-0 flex-1">
                <span class={cn('block truncate text-sm font-medium', selectedKey === o.owner ? 'text-fg' : 'text-muted')}>{o.label}</span>
                <span class="block truncate text-[11px] text-muted">{meta?.role ?? o.model ?? ''}</span>
              </span>
              {#if openCount > 0}<Chip tone="warn">{openCount}</Chip>{/if}
              {#if !o.canEdit}<Lock size={12} class="shrink-0 text-muted" />{/if}
            </button>
          {/each}
          <div class="my-2 border-t border-line"></div>
          <button
            type="button"
            onclick={() => select('shared')}
            class={cn(
              'flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left transition-colors',
              selectedKey === 'shared' ? 'bg-raised' : 'hover:bg-hover',
            )}
          >
            <span class="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-line text-sm text-muted">✦</span>
            <span class="min-w-0 flex-1">
              <span class={cn('block truncate text-sm font-medium', selectedKey === 'shared' ? 'text-fg' : 'text-muted')}>Every agent</span>
              <span class="block text-[11px] text-muted">shared know-how</span>
            </span>
            {#if shared && !shared.canEdit}<Lock size={12} class="shrink-0 text-muted" />{/if}
          </button>
        </aside>

        <!-- ── The selected agent's world ── -->
        {#if selected}
          <main class="min-w-0 space-y-8">
            <div class="flex items-center gap-4">
              {#if selected.owner === 'shared'}
                <span class="flex h-14 w-14 items-center justify-center rounded-lg border border-line bg-raised text-2xl text-muted">✦</span>
              {:else}
                <Avatar name={selected.label} class="h-14 w-14 text-lg" />
              {/if}
              <div class="min-w-0 flex-1">
                <h2 class="truncate text-xl font-semibold text-fg">{selected.owner === 'shared' ? 'Every agent' : selected.label}</h2>
                <p class="truncate text-sm text-muted">
                  {selected.owner === 'shared'
                    ? 'Know-how every agent in the fleet carries.'
                    : (agentMeta(selected)?.role ?? selected.model ?? '')}
                </p>
              </div>
              {#if selected.canEdit}
                <Button onclick={() => selected && (guide = { owner: selected.owner })}>
                  Teach {selected.owner === 'shared' ? 'everyone' : firstName(selected)}
                </Button>
              {/if}
            </div>

            <!-- Suggestions — what the agent asked for -->
            {#if ownerGaps(selected).length > 0}
              <section>
                <SectionTitle
                  title={selected.owner === 'shared' ? 'Agents asked for help with' : `${firstName(selected)} asked for help with`}
                  hint="Work it hit and couldn't do properly — build the know-how and it won't happen again."
                />
                <div class="space-y-2">
                  {#each ownerGaps(selected) as g (g.id)}
                    <Panel class="border-warning/25">
                      <div class="flex items-center gap-2">
                        <span class="text-sm font-medium text-fg">{g.kind.replace(/-/g, ' ')}</span>
                        <Chip tone="warn">{g.seenCount}×</Chip>
                        {#if selected.owner === 'shared'}<span class="text-xs text-muted">{g.agentModel}</span>{/if}
                        <div class="ml-auto flex items-center gap-1.5">
                          {#if selected.canEdit}
                            <Button
                              size="sm"
                              onclick={() =>
                                selected &&
                                (guide = {
                                  owner: selected.owner,
                                  gapId: g.id,
                                  name: g.kind.replace(/-/g, ' '),
                                  describe: `${g.missing}${g.needs ? `\n\nWhat the reporting agent said a flow would need:\n${g.needs}` : ''}`,
                                  boardIds: g.boardId ? [g.boardId] : [],
                                })}
                            >
                              Build it
                            </Button>
                          {/if}
                          <Button
                            size="sm"
                            variant="ghost"
                            onclick={() => void setGapStatus(g.id, 'dismissed').then(() => qc.invalidateQueries({ queryKey: ['gaps', 'open'] }))}
                          >
                            Dismiss
                          </Button>
                        </div>
                      </div>
                      <p class="mt-1 text-sm text-muted">{g.missing}</p>
                    </Panel>
                  {/each}
                </div>
              </section>
            {/if}

            <!-- Knows — skills -->
            <section>
              {#snippet addSkillAction()}
                <Button size="sm" variant="outline" onclick={() => selected && (guide = { owner: selected.owner })}>
                  <Plus size={14} />
                </Button>
              {/snippet}
              <SectionTitle
                title={selected.owner === 'shared' ? 'Everyone knows' : `${firstName(selected)} knows`}
                hint="Skills — the how. Agents read these live; edits apply on their next run."
                action={selected.canEdit ? addSkillAction : undefined}
              />
              {#if selected.skills.length === 0 && (selected.owner === 'shared' || sharedSkills.size === 0)}
                <Panel>
                  <EmptyState
                    icon="✦"
                    title="Nothing yet"
                    hint={selected.canEdit ? '“Teach” walks you through the first one — name the work, explain it in plain words, Muse drafts the skill.' : 'Nothing taught yet.'}
                  />
                </Panel>
              {:else}
                <Panel class="p-0">
                  <div class="divide-y divide-line">
                    {#each selected.skills as s (s.name)}
                      <SkillRow
                        owner={selected.owner}
                        skill={s}
                        {owners}
                        canEdit={selected.canEdit && (!s.platform || isAdmin)}
                        onOpen={() => selected && void navigate('/studio', { search: { a: selectedKey, sk: `${selected.owner}/${s.name}` } })}
                      />
                    {/each}
                    {#if selected.owner !== 'shared'}
                      {#each shared?.skills ?? [] as s (`shared/${s.name}`)}
                        <SkillRow
                          owner="shared"
                          skill={s}
                          {owners}
                          sharedBadge
                          canEdit={(shared?.canEdit ?? false) && (!s.platform || isAdmin)}
                          onOpen={() => void navigate('/studio', { search: { a: selectedKey, sk: `shared/${s.name}` } })}
                        />
                      {/each}
                    {/if}
                  </div>
                </Panel>
              {/if}
            </section>

            <!-- Routed here — workflows -->
            <section>
              <SectionTitle
                title={selected.owner === 'shared' ? 'Routing everyone can serve' : `Work routed to ${firstName(selected)}`}
                hint="Workflows — the which. When a ticket matches, its skills ride along on dispatch."
              />
              {#if routedTo(selected).length === 0}
                <Panel>
                  <p class="py-1 text-sm text-muted">
                    No routing yet.{selected.canEdit ? ' Add recognition rules in “Teach” and matching tickets will carry the flow automatically.' : ''}
                  </p>
                </Panel>
              {:else}
                <Panel class="p-0">
                  <div class="divide-y divide-line">
                    {#each routedTo(selected) as w (w.id)}
                      <button
                        type="button"
                        onclick={() => void navigate('/studio', { search: { a: selectedKey, w: w.id } })}
                        class="flex w-full items-center gap-3 px-5 py-3 text-left transition-colors hover:bg-hover"
                      >
                        <span class="min-w-0 flex-1">
                          <span class="block truncate text-sm font-medium text-fg">{w.name}</span>
                          <span class="block truncate text-sm text-muted">{describe(w)}</span>
                        </span>
                        {#if !w.enabled}<Chip>off</Chip>{/if}
                      </button>
                    {/each}
                  </div>
                </Panel>
              {/if}
            </section>
          </main>
        {:else if libraryQuery.isError && libraryQuery.data === undefined}
          <!-- Nothing to select because the library read FAILED, not because
               the fleet is empty. Name the real problem. -->
          <QueryError
            error={libraryQuery.error}
            title="Could not load the skill library"
            onRetry={() => void libraryQuery.refetch()}
          />
        {:else}
          <EmptyState icon="◍" title="No agents yet" hint="The Studio lights up once the fleet has agents." />
        {/if}
      </div>
    </div>

    <!-- Overlays -->
    {#if skOwner && skName && skOwnerInfo}
      <SkillEditor
        owner={skOwner}
        ownerLabel={skOwnerInfo.label}
        name={skName}
        canEdit={skOwnerInfo.canEdit && (!skOwnerInfo.skills.find((x) => x.name === skName)?.platform || isAdmin)}
        onClose={closeOverlay}
        onChanged={() => void qc.invalidateQueries({ queryKey: ['skill-library'] })}
      />
    {/if}
    {#if openWorkflow}
      <Modal open onClose={closeOverlay} width="max-w-2xl" title={openWorkflow.name}>
        {#key openWorkflow.id}
          <WorkflowDetail
            workflow={openWorkflow}
            onChanged={() => void qc.invalidateQueries({ queryKey: ['workflows'] })}
            onDelete={() => openWorkflow && void removeWorkflow(openWorkflow)}
          />
        {/key}
      </Modal>
    {/if}
    {#if guide}
      <StudioGuide
        open
        onClose={() => (guide = null)}
        {owners}
        prefill={guide}
        onCreated={() => {
          // A gap resolves only once a human actually ratified a skill for
          // it — cancelling the guide leaves the suggestion in place.
          if (guide?.gapId) void setGapStatus(guide.gapId, 'resolved').then(() => qc.invalidateQueries({ queryKey: ['gaps', 'open'] }))
        }}
      />
    {/if}
  </div>
{/if}
