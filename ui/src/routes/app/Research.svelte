<script lang="ts">
  import { pathId } from '@/lib/route-tabs'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { Gauge, Trash2 } from '@lucide/svelte'
  import { navigate, route } from '@/router'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import RailSurface from '@/components/app/RailSurface.svelte'
  import Rail from '@/components/app/Rail.svelte'
  import Stage from '@/components/app/Stage.svelte'
  import StageHeader from '@/components/app/StageHeader.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import SendButton from '@/components/chat/SendButton.svelte'
  import AgentPicker from '@/components/chat/AgentPicker.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import KeyHint from '@/components/ui/KeyHint.svelte'
  import { cn } from '@/lib/cn'
  import { slide } from '@/lib/motion'
  import Materialize from '@/components/ui/Materialize.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useAgents } from '@/lib/agents'
  import { useHasPerm, useSession } from '@/lib/session'
  import { useStickyAgent } from '@/lib/sticky-agent.svelte'
  import { relativeTime } from '@/lib/fleet'
  import NoModelBump from '@/components/setup/NoModelBump.svelte'
  import {
    deleteResearch,
    MODE_META,
    startResearch,
    useResearchRuns,
    type ResearchMode,
    type ResearchRun,
  } from '@/lib/research'
  import RunView from './research/RunView.svelte'

  // Research — Perplexity-grade cited research, run by YOUR agents. Ask a
  // question, pick a depth (Recon / Brief / Expedition) and whose expertise
  // should drive it; the pipeline runs server-side and lands an org-visible,
  // fully cited report document. Everything indexes into the activity brain so
  // chats, plans, and boards can pull from it later.

  const STATUS_DOT: Record<ResearchRun['status'], DotStatus> = {
    queued: 'idle',
    running: 'accent',
    done: 'ok',
    error: 'danger',
  }

  const qc = useQueryClient()
  const sessionQuery = useSession()
  const session = $derived(sessionQuery.data)
  const fleetQuery = useAgents()
  const agentsLoading = $derived(fleetQuery.isLoading)
  const agents = $derived(fleetQuery.data?.agents ?? [])
  const runsQuery = useResearchRuns()
  const runs = $derived(runsQuery.data ?? [])
  const runsLoading = $derived(runsQuery.isLoading)

  // THE URL IS THE SELECTION: /research/<runId> IS the selected run, and a
  // completion notification links straight at it. It was `?r=<runId>`; a run is
  // what the page is ABOUT, so it belongs in the path.
  const selectedId = $derived(pathId(route.pathname, '/research'))
  const setSelectedId = (id: string | null) => {
    if (id) void navigate('/research/:runId', { params: { runId: id } })
    else void navigate('/research')
  }
  let question = $state('')
  /** Measured composer height — what the content below reserves. */
  let composerH = $state(0)
  const mayRun = useHasPerm('research.run')
  let mode = $state<ResearchMode>('brief')
  const sticky = useStickyAgent('research', () => agents)
  const agent = $derived(sticky.selected)
  const pickAgent = sticky.select
  let starting = $state(false)
  let error = $state<string | null>(null)
  const menu = useContextMenu()

  const start = async () => {
    if (!question.trim() || !agent) return
    starting = true
    error = null
    try {
      const run = await startResearch(question.trim(), mode, agent)
      question = ''
      setSelectedId(run.id)
      void qc.invalidateQueries({ queryKey: ['research-runs'] })
    } catch (e) {
      error = (e as Error).message
    } finally {
      starting = false
    }
  }

  const remove = async (run: ResearchRun) => {
    if (!(await confirm({ title: 'Remove run', message: `Remove "${run.question.slice(0, 80)}" from the list? The report document (if any) stays in Artifacts.`, confirmLabel: 'Remove' }))) return
    await deleteResearch(run.id)
    if (selectedId === run.id) setSelectedId(null)
    void qc.invalidateQueries({ queryKey: ['research-runs'] })
  }

  const canDelete = (run: ResearchRun) => run.ownerUserId === session?.id || session?.role === 'admin'

  const selected = $derived(runs.find((r) => r.id === selectedId) ?? null)
</script>

{#snippet stageHeader()}
  {#if selected}
    {#snippet headerActions()}
      {#if selected && canDelete(selected)}
        <DangerLink onClick={() => selected && void remove(selected)}>Remove</DangerLink>
      {/if}
    {/snippet}
    <StageHeader
      title={selected.title ?? selected.question}
      meta={`${MODE_META[selected.mode].label} · ${selected.agentModel}`}
      actions={canDelete(selected) ? headerActions : undefined}
    />
  {/if}
{/snippet}

<RailSurface>
  <Rail title="Research">
    <div class="mb-3">
      <AgentPicker {agents} value={agent} onChange={pickAgent} loading={agentsLoading} fullWidth />
    </div>
    <!-- Skeleton → content as one motion: run-row-shaped skeletons (dot +
         title line, then the chip/meta line) materialize into the real rows.
         Materialize direct (no QueryState here): error and resolved-empty
         stay in the content branch, exactly as this site already keyed them. -->
    <Materialize loading={runsLoading} count={5} class="space-y-0.5">
      {#snippet skeleton(i)}
        <div aria-hidden="true" class="rounded-md px-2 py-1.5">
          <div class="flex h-5 items-center gap-2">
            <!-- Fixed geometry, so nothing about it is uncertain — a flat rail, not a skeleton (UI-CONVENTIONS, Loading). -->
            <div class="h-1.5 w-1.5 shrink-0 rounded-full bg-line"></div>
            <Skeleton class={`h-3 rounded-full ${['w-4/5', 'w-3/5', 'w-11/12', 'w-1/2', 'w-2/3'][i % 5]}`} />
          </div>
          <div class="mt-0.5 flex h-4 items-center gap-2 pl-3.5">
            <Skeleton class="h-3 w-12 rounded" />
            <Skeleton class="h-2.5 w-16 rounded-full" />
          </div>
        </div>
      {/snippet}
      {#if runsQuery.isError && runsQuery.data === undefined}
        <!-- The run list is someone's research history. "No research yet" over
             a failed read invites them to re-run work they already paid for. -->
        <QueryError
          variant="compact"
          error={runsQuery.error}
          title="Could not load your research"
          onRetry={() => void runsQuery.refetch()}
        />
      {:else if runs.length === 0}
        <EmptyState variant="compact" icon="◎" title="No research yet." hint="Ask something worth knowing." />
      {:else}
        {#each runs as r (r.id)}
          <button
            type="button"
            onclick={() => setSelectedId(r.id)}
            oncontextmenu={(e) =>
              menu.openMenu(e, [
                { label: 'Open', onSelect: () => setSelectedId(r.id) },
                { label: 'Copy link', onSelect: () => copyAppLink(`/research/${r.id}`) },
                ...(canDelete(r)
                  ? (['sep', { label: 'Remove', danger: true, onSelect: () => void remove(r) }] as ContextMenuEntry[])
                  : []),
              ])}
            class={cn(
              'group block w-full rounded-md px-2 py-1.5 text-left transition-colors dither-fill',
              selectedId === r.id ? 'bg-card' : '',
            )}
          >
            <div class="flex items-center gap-2">
              <StatusDot status={STATUS_DOT[r.status]} pulse={r.status === 'running'} class="h-1.5 w-1.5" />
              <span class="min-w-0 flex-1 truncate text-sm text-fg">{r.title ?? r.question}</span>
              {#if canDelete(r)}
                <Trash2
                  size={13}
                  class="hidden shrink-0 text-muted transition-colors hover:text-danger group-hover:block"
                  onclick={(e: MouseEvent) => {
                    e.stopPropagation()
                    void remove(r)
                  }}
                />
              {/if}
            </div>
            <!-- §10 session-row meta: 10px mono chrome voice. -->
            <div class="mt-0.5 flex items-center gap-2 pl-3.5 font-mono text-[10px] tracking-[0.05em] text-muted">
              <Chip>{MODE_META[r.mode].label}</Chip>
              <span class="truncate">{r.requestedBy}</span>
              <span class="ml-auto shrink-0">{relativeTime(r.createdAt)}</span>
            </div>
          </button>
        {/each}
      {/if}
    </Materialize>
  </Rail>

  <Stage header={stageHeader}>
    <!-- The ask FLOATS over the content so the report and the zero state both
         own the whole stage. Its measured height rides down as
         `--research-composer`; the report column and the discussion's own chat
         input each reserve that much. Measured, not a constant — the textarea
         auto-grows. -->
    <div class="relative flex h-full min-h-0 flex-col" style:--research-composer="{composerH}px">
      <!-- Research cannot run without a provider; the surface still opens and
           past runs still read. Renders nothing once a provider exists. -->
      <NoModelBump class="m-4 shrink-0" />
      <!-- flex-col so RunView's `flex min-h-0 flex-1` root fills it and its two
           columns scroll independently. -->
      <div class="flex min-h-0 flex-1 flex-col">
        {#if selectedId}
          <RunView runId={selectedId} />
        {:else}
          <!-- Edge to edge: no centring wrapper. EmptyState's `full` variant
               centres its own words and paints its vignette to its bounds. -->
          <EmptyState
            class="flex-1"
            icon="◎"
            title="Research"
            hint="Recon answers fast; Brief maps a topic; Expedition goes deep. Reports are cited, org-visible documents your agents can retrieve later."
          />
        {/if}
      </div>

      <!-- pointer-events-none gutter, auto panel: the float spans the stage so
           the panel can centre in it, and without that split the transparent
           margin would swallow clicks on the report behind it. Opaque ground,
           or the report shows through beneath the panel. -->
      <div bind:clientHeight={composerH} class="pointer-events-none absolute inset-x-0 bottom-0 z-10 bg-surface pb-6 pt-2">
        <div class="mx-auto w-full max-w-[var(--converse-width)] px-6">
        <!-- §7 composer anatomy: panel body, STRONG hairline, radius 8,
             p-2, matte float shadow — with the prompt in a ground inset. -->
        <div class="pointer-events-auto rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]">
          <div class="flex items-end gap-2">
            <div class="relative min-w-0 flex-1 rounded-md border border-line bg-surface px-2 py-1">
              <Textarea
                autoGrow
                rows={1}
                bind:value={question}
                disabled={!mayRun.current}
                placeholder={mayRun.current ? 'What should we find out?' : 'You don’t have permission to run research'}
                class="max-h-40 min-h-[2.75rem] border-0 bg-transparent pr-12 focus:border-0"
                onkeydown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault()
                    void start()
                  }
                }}
              />
              <!-- §7 signature affordance: gold send tile inside the well,
                   top-right — mouse/touch path to start a run. -->
              <SendButton
                class="absolute right-2 top-2"
                title="Start (⏎)"
                enabled={mayRun.current && !starting && !!question.trim() && !!agent}
                onClick={() => void start()}
              />
            </div>
            <KeyHint keys="⏎" label="start" visible={!!question.trim() && !!agent} class="self-end mb-3" />
            <ComposerPicker
              icon={Gauge}
              value={mode}
              onChange={(m) => (mode = m as ResearchMode)}
              title="Research depth"
              menuLabel="Depth"
              options={(Object.keys(MODE_META) as ResearchMode[]).map((m) => ({
                value: m,
                label: MODE_META[m].label,
                sub: `${MODE_META[m].tagline} · ${MODE_META[m].eta}`,
              }))}
            />
            {#if starting}
              <span class="grid h-9 w-9 shrink-0 place-items-center self-end mb-1"><WaitingMark site="research/start" size={12} class="text-accent" /></span>
            {/if}
          </div>
          {#if error}<div transition:slide={{ duration: 150 }} class="px-2 pb-1 pt-1 text-xs text-danger">{error}</div>{/if}
        </div>
        </div>
      </div>
    </div>
  </Stage>
  <ContextMenu {menu} />
</RailSurface>
