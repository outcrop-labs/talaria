<script lang="ts">
  import SettingsPage from '@/components/settings/SettingsPage.svelte'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronDown, Users } from '@lucide/svelte'
  import { navigate, p } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { toastError } from '@/lib/toast.svelte'
  import WorkflowDialog from '@/components/workflows/WorkflowDialog.svelte'
  import SkillEditor from '@/components/skills/SkillEditor.svelte'
  import { skillTitle } from '@/components/skills/skill-title'
  import StudioGuide, { type GuidePrefill } from '@/components/workflows/StudioGuide.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { staggerIn } from '@/lib/motion'
  import { useAgents } from '@/lib/agents'
  import { useSession } from '@/lib/session'
  import { useBoards } from '@/lib/boards.svelte'
  import {
    deleteWorkflow,
    setGapStatus,
    useGaps,
    useWorkflows,
    type CapabilityGap,
    type TaskWorkflow,
  } from '@/lib/workflows'
  import { SKILLS_KEY, useSkills, type SkillOwner } from '@/lib/skills'
  import AgentPicker from './studio/AgentPicker.svelte'
  import SkillMarketplaceModal from './studio/SkillMarketplaceModal.svelte'
  import SkillRow from './studio/SkillRow.svelte'

  // The Studio — build your agents, one at a time. The rail picks who you're
  // building for ("Every agent" = shared know-how, or one agent); the dashboard
  // is that agent's whole working world:
  //   Asked for help   what IT asked for (capability gaps, ranked by recurrence),
  //                    first when there is any and absent when there is none
  //   Skills           what it knows — own + inherited shared know-how
  //   Routed work      the workflows that steer ticket work onto it
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
  const libraryQuery = useSkills()
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
    { key: 'workflows', title: 'Could not load workflows: routed work is incomplete', query: workflowsQuery },
    { key: 'gaps', title: 'Could not load requests: what the agents asked for is missing', query: gapsQuery },
    { key: 'agents', title: 'Could not load your agents: their roles fall back to model ids', query: agentsQuery },
    { key: 'boards', title: 'Could not load boards: workflow rules name board ids instead', query: boardsQuery },
  ]
  const sideFailures = $derived(sideReads.filter((r) => r.query.isError))
  const sessionQuery = useSession()
  const isAdmin = $derived(sessionQuery.data?.role === 'admin')
  let guide = $state<(GuidePrefill & { gapId?: string }) | null>(null)
  let market = $state(false)


  const agents = $derived(owners.filter((o) => o.owner !== 'shared'))
  const shared = $derived(owners.find((o) => o.owner === 'shared'))
  const selectedKey = $derived(sp('a') ?? agents.find((o) => o.canEdit)?.owner ?? agents[0]?.owner ?? 'shared')
  const selected = $derived(owners.find((o) => o.owner === selectedKey) ?? shared ?? null)
  const select = (a: string) => void navigate('/studio', { search: { a } })
  // A row that opens a skill or a route is a link, so it opens in a new tab like any other.
  const studioHref = (search: Record<string, string>) => `${p('/studio')}?${new URLSearchParams(search)}`

  const sharedSkills = $derived(new Set(shared?.skills.map((s) => s.name) ?? []))
  const boardName = (id: string) => boards.find((b) => b.id === id)?.name ?? 'a board'
  const agentMeta = (o: SkillOwner) => agentsData?.agents.find((a) => a.id === o.model)
  // How the page refers to the agent in a sentence. A long name is a title, not
  // a name ("Carrier Compliance & Insurance Certificate Reviewer"), and its
  // first word alone is the wrong name, so the sentence says "this agent".
  const callName = (o: SkillOwner) => (o.label.length <= 18 ? o.label : 'this agent')
  // An agent with no role shows none. Its model id stands in only when the roles could not be read at all.
  const roleOf = (o: SkillOwner) => agentMeta(o)?.role ?? (agentsQuery.isError ? (o.model ?? '') : '')
  /** Who a request came from, by the name people know the agent by. */
  const askerOf = (g: CapabilityGap) => owners.find((o) => o.model === g.agentModel)?.label ?? g.agentModel
  const times = (n: number) => (n === 1 ? 'once' : `${n} times`)
  const or = (items: string[]) => items.join(' or ')

  /** Workflows steering work onto this owner: any bound skill it carries.
   *  The shared view also collects workflows whose skills nobody carries. */
  const routedTo = (o: SkillOwner): TaskWorkflow[] => {
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
      w.match.boards?.length ? `on ${or(w.match.boards.map(boardName))}` : '',
      w.match.labels?.length ? `labeled ${or(w.match.labels.map((l) => `“${l}”`))}` : '',
      w.match.keywords?.length ? `mentioning ${or(w.match.keywords.map((k) => `“${k}”`))}` : '',
    ]
      .filter(Boolean)
      .join(', ')
    if (!where) return 'No rules yet, so no ticket matches.'
    return w.skills.length ? `Tickets ${where} follow ${w.skills.map(skillTitle).join(' and ')}.` : `Tickets ${where} match, but no skill is attached yet.`
  }

  const ownerGaps = (o: SkillOwner): CapabilityGap[] =>
    o.owner === 'shared' ? gaps : gaps.filter((g) => g.agentModel === o.model)

  const teach = (o: SkillOwner) => (guide = { owner: o.owner })
  const build = (o: SkillOwner, g: CapabilityGap) =>
    (guide = {
      owner: o.owner,
      gapId: g.id,
      name: g.kind.replace(/-/g, ' '),
      describe: `${g.missing}${g.needs ? `\n\nWhat the reporting agent said a flow would need:\n${g.needs}` : ''}`,
      boardIds: g.boardId ? [g.boardId] : [],
    })
  const dismiss = (g: CapabilityGap) =>
    void setGapStatus(g.id, 'dismissed').then(() => qc.invalidateQueries({ queryKey: ['gaps', 'open'] }))

  // Overlays driven by URL params
  const skOwner = $derived(sp('sk')?.split('/', 2)[0])
  const skName = $derived(sp('sk')?.split('/', 2)[1])
  const skOwnerInfo = $derived(owners.find((o) => o.owner === skOwner))
  const openWorkflow = $derived(workflows.find((w) => w.id === sp('w')) ?? null)
  const closeOverlay = () => void navigate('/studio', { search: { a: selectedKey } })

  const removeWorkflow = async (w: TaskWorkflow) => {
    if (!(await confirm({ title: 'Delete workflow', message: `Delete "${w.name}"? Matching tickets dispatch without its skills.`, confirmLabel: 'Delete', danger: true }))) return
    try {
      // Already gone (another admin deleted it) resolves: close and refresh.
      await deleteWorkflow(w.id)
    } catch (e) {
      toastError('Delete failed', e)
      return
    }
    closeOverlay()
    await qc.invalidateQueries({ queryKey: ['workflows'] })
  }
</script>

<!-- A section with nothing in it is its title and one line, not an empty card. -->
{#snippet quiet(title: string, line: string)}
  <section>
    <h2 class="font-sans text-[15px] font-semibold leading-5 text-fg">{title}</h2>
    <p class="mt-1 font-sans text-[13px] leading-5 text-muted">{line}</p>
  </section>
{/snippet}

<SettingsPage title="Agent Studio" blurb="Teach an agent a new skill and review what it knows.">
  {#snippet actions()}
    {#if selected?.canEdit}
      <Button size="sm" onclick={() => selected && teach(selected)}>
        Teach {selected.owner === 'shared' ? 'everyone' : callName(selected)}
      </Button>
    {/if}
  {/snippet}
{#if isLoading}
    <SkeletonRows rows={8} avatar />
{:else}
    <!-- Page content entrance AND post-skeleton reveal in one: this branch
         mounts when the library lands, so failures → rail+world grid rise in
         sequence (ANIMATIONS.md). Never on the skeleton branch. -->
    <div use:staggerIn>
      {#if sideFailures.length > 0}
        <SettingsGroup class="mb-10">
          {#each sideFailures as r (r.key)}
            <div class="px-5 py-4">
              <QueryError
                variant="inline"
                title={r.title}
                error={r.query.error}
                onRetry={() => void r.query.refetch()}
              />
            </div>
          {/each}
        </SettingsGroup>
      {/if}

      <!-- The rail+world grid is the section whose items cascade: the frame
           fades while the rail's rows and the world's blocks (agent header,
           Asked for help, Skills, Routed work) rise 30ms apart, capped —
           staggerIn's data-stagger-items contract. -->
      <div class="grid gap-8 lg:grid-cols-[17rem_minmax(0,1fr)]" data-stagger-items="aside > *, main > *">
        <AgentPicker {agents} {shared} {selectedKey} {roleOf} openCount={(o) => ownerGaps(o).length} onSelect={select} />

        <!-- ── The selected agent's world ── -->
        {#if selected}
          {@const everyone = selected.owner === 'shared'}
          {@const who = everyone ? 'every agent' : callName(selected)}
          {@const requests = ownerGaps(selected)}
          {@const routed = routedTo(selected)}
          <main class="min-w-0 space-y-8">
            <div class="flex items-center gap-3">
              {#if everyone}
                <span class="grid h-10 w-10 shrink-0 place-items-center rounded-full border border-line-strong bg-raised text-muted">
                  <Users size={18} />
                </span>
              {:else}
                <Avatar name={selected.label} class="h-10 w-10 text-sm" />
              {/if}
              <div class="min-w-0 flex-1">
                <div class="flex items-center gap-2">
                  <h2 class="min-w-0 break-words font-sans text-lg font-semibold leading-6 text-fg">{everyone ? 'Every agent' : selected.label}</h2>
                  {#if !selected.canEdit}<Chip>View only</Chip>{/if}
                </div>
                {#if everyone || roleOf(selected)}
                  <p class="font-sans text-[13px] leading-5 text-muted">
                    {everyone ? 'Skills here reach every agent on the team.' : roleOf(selected)}
                  </p>
                {/if}
              </div>
            </div>

            {#if requests.length}
              <SettingsSection
                title="Asked for help"
                description={everyone
                  ? 'Work an agent could not do properly. Build the skill and it stops asking.'
                  : `Work ${who} could not do properly. Build the skill and it stops asking.`}
              >
                <SettingsGroup>
                  {#each requests as g (g.id)}
                    {@const kind = skillTitle(g.kind)}
                    <SettingsRow label={kind} info={g.needs ? `What it says it needs: ${g.needs}` : undefined}>
                      {#snippet meta()}
                        <span class="min-w-0 truncate font-sans text-[13px] leading-5 text-muted">
                          {everyone ? `${askerOf(g)} asked` : 'Asked'} {times(g.seenCount)}, last {relativeTime(g.lastSeen)}
                        </span>
                      {/snippet}
                      {#snippet hint()}<span class="line-clamp-2" title={g.missing}>{g.missing}</span>{/snippet}
                      {#if selected.canEdit}
                        <Button size="sm" variant="outline" onclick={() => selected && build(selected, g)}>Build it</Button>
                      {/if}
                      <SettingsRowMenu label={`Actions for ${kind}`} items={[{ label: 'Dismiss', onSelect: () => dismiss(g) }]} />
                    </SettingsRow>
                  {/each}
                </SettingsGroup>
              </SettingsSection>
            {/if}

            <SettingsSection
              title="Skills"
              description={everyone ? 'What every agent knows. Edits apply on their next run.' : `What ${who} knows. Edits apply on its next run.`}
            >
              {#snippet action()}
                {#if selected?.canEdit}
                  <DropdownMenu
                    items={[
                      { label: 'New skill', onSelect: () => selected && teach(selected) },
                      { label: 'Browse marketplace', onSelect: () => (market = true) },
                    ]}
                  >
                    {#snippet trigger(open)}
                      <Button size="sm" variant="outline" aria-haspopup="menu" aria-expanded={open}>
                        Add skill <ChevronDown size={14} class="-mr-0.5 text-muted" />
                      </Button>
                    {/snippet}
                  </DropdownMenu>
                {/if}
              {/snippet}
              <SettingsGroup>
                {#if selected.skills.length === 0 && (everyone || sharedSkills.size === 0)}
                  <EmptyState
                    variant="compact"
                    icon="✦"
                    title="No skills yet"
                    hint={selected.canEdit ? 'Name the work and explain it in plain words. Muse drafts the skill.' : 'Nothing taught yet.'}
                  />
                {:else}
                  {#each selected.skills as s (s.name)}
                    <SkillRow
                      owner={selected.owner}
                      skill={s}
                      {owners}
                      canEdit={(selected.canEdit ?? false) && (!s.platform || isAdmin)}
                      href={studioHref({ a: selectedKey, sk: `${selected.owner}/${s.name}` })}
                    />
                  {/each}
                  {#if !everyone}
                    {#each shared?.skills ?? [] as s (`shared/${s.name}`)}
                      <SkillRow
                        owner="shared"
                        skill={s}
                        {owners}
                        sharedBadge
                        canEdit={(shared?.canEdit ?? false) && (!s.platform || isAdmin)}
                        href={studioHref({ a: selectedKey, sk: `shared/${s.name}` })}
                      />
                    {/each}
                  {/if}
                {/if}
              </SettingsGroup>
            </SettingsSection>

            {#if routed.length}
              <SettingsSection
                title="Routed work"
                description={everyone ? 'Tickets any agent can take, and the skills they arrive with.' : `Tickets sent to ${who}, and the skills they arrive with.`}
              >
                <SettingsGroup>
                  {#each routed as w (w.id)}
                    <SettingsRow label={w.name} hint={describe(w)} openHref={studioHref({ a: selectedKey, w: w.id })} openLabel={`Edit ${w.name}`}>
                      {#snippet status()}{#if !w.enabled}<StatusDot status="idle" />Off{/if}{/snippet}
                    </SettingsRow>
                  {/each}
                </SettingsGroup>
              </SettingsSection>
            {:else}
              {@render quiet('Routed work', selected.canEdit ? 'No routing yet. Teach a skill with rules and matching tickets carry it.' : 'No routing yet')}
            {/if}
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
          <EmptyState icon="◍" title="No agents yet" hint="The Studio lights up once you have agents." />
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
        onChanged={() => void qc.invalidateQueries({ queryKey: SKILLS_KEY })}
      />
    {/if}
    {#if openWorkflow}
      <WorkflowDialog
        workflow={openWorkflow}
        onClose={closeOverlay}
        onChanged={() => void qc.invalidateQueries({ queryKey: ['workflows'] })}
        onDelete={() => openWorkflow && void removeWorkflow(openWorkflow)}
      />
    {/if}
    {#if market && selected?.canEdit}
      <SkillMarketplaceModal owner={selected.owner} owners={rawOwners} onClose={() => (market = false)} />
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
{/if}
</SettingsPage>
