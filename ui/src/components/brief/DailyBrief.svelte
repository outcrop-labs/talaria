<script lang="ts">
  import { getContext, tick, type Snippet } from 'svelte'
  import { Activity, CalendarDays, ExternalLink, Mail, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import InboxBoard from '@/components/inbox-board/InboxBoard.svelte'
  import InboxDrawer from '@/components/inbox-board/InboxDrawer.svelte'
  import InboxMessages from '@/components/inbox-board/InboxMessages.svelte'
  import InboxToday from '@/components/inbox-board/InboxToday.svelte'
  import { LINK, useInboxBoard } from '@/components/inbox-board/inbox-board.svelte'
  import { laneTally, todayMessages } from '@/lib/inbox-board'
  import { cn } from '@/lib/cn'
  import { fly, PANEL_X, staggerIn } from '@/lib/motion'
  import { navigate } from '@/router'
  import BriefAbsentState from './BriefAbsent.svelte'
  import BriefSkeleton from './BriefSkeleton.svelte'
  import BriefTimeline from './BriefTimeline.svelte'
  import { INBOX_FOCUS_WORKSPACE_KEY, type InboxFocusWorkspaceValue } from '@/components/inbox/inbox-focus-shell'
  import { greeting } from '@/routes/app/home/home'
  import { useSession } from '@/lib/session'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import { isBriefAbsent, useBrief, useBriefActions, useBriefLive, type BriefLine, type BriefView } from './daily-brief.svelte'

  /**
   * FAILURE MODES. This page must never render calm over a read it could not make:
   *   · a failed BACKGROUND refetch keeps the document and says it is stale.
   *   · a failed FIRST read renders the error, never an empty day.
   */
  let { mail, agenda }: { mail: Snippet; agenda: Snippet } = $props()

  const query = useBrief()
  const actions = useBriefActions()
  useBriefLive()
  const session = useSession()

  claimViewTitle(greeting(session.data?.name ?? session.data?.email))
  // May be absent only in tests; the layout always provides the shell.
  const workspace = getContext<InboxFocusWorkspaceValue | undefined>(INBOX_FOCUS_WORKSPACE_KEY)

  type Aside = 'timeline' | 'mail' | 'agenda'
  let aside = $state<Aside | null>(null)

  const brief = $derived(query.data && !isBriefAbsent(query.data) ? (query.data as BriefView) : null)
  const absent = $derived(query.data && isBriefAbsent(query.data) ? query.data : null)
  const failed = $derived(query.isError && query.data === undefined)
  const stale = $derived(query.isError && query.data !== undefined)

  const board = useInboxBoard(
    () => brief,
    () => query.data !== undefined || query.isError,
  )

  const messages = $derived(todayMessages(brief))
  const calm = $derived(!board.pending && !board.failure && !failed && board.lanes.needs.length === 0 && messages.length === 0)

  let inbox = $state<InboxBoard | null>(null)
  let drawerKey = $state<string | null>(null)
  let drawerFull = $state(false)
  const drawerCard = $derived(drawerKey ? (board.cards.find((c) => c.key === drawerKey) ?? null) : null)
  let drawerBeside = $state(false)

  function openDrawer(key: string): void {
    drawerKey = key
    drawerBeside = true
  }

  async function closeDrawer(): Promise<void> {
    const key = drawerKey
    drawerKey = null
    drawerFull = false
    await tick()
    if (key) document.getElementById(`inbox-card:${key}`)?.focus()
  }

  const FOCUS_SOURCE_TYPES = new Set(['task', 'notification', 'channel', 'approval'])

  function ask(line: BriefLine): void {
    const key = line.current.sourceType && FOCUS_SOURCE_TYPES.has(line.current.sourceType) ? line.key : null
    workspace?.askAbout({ key, question: line.current.title })
  }

  function toggleAside(next: Aside): void {
    aside = aside === next ? null : next
    if (aside === 'timeline' && brief) actions.markRead(brief)
  }
</script>

{#snippet tool(id: Aside, Icon: typeof Activity, label: string, count = 0)}
  <Button variant="link" aria-pressed={aside === id} class="group gap-1.5 text-[13px] text-fg hover:no-underline" onclick={() => toggleAside(id)}>
    <Icon size={13} />
    <span class={cn(LINK, 'group-hover:underline group-focus-visible:underline', aside === id && 'underline')}>{label}</span>
    {#if count > 0}<span class="font-normal tabular-nums text-fg">{count}<span class="sr-only"> unseen</span></span>{/if}
  </Button>
{/snippet}

{#snippet tools()}
  {#if brief}{@render tool('timeline', Activity, 'Timeline', brief.unseenCount)}{/if}
  {@render tool('mail', Mail, 'Mail')}
  {@render tool('agenda', CalendarDays, 'Agenda')}
{/snippet}

{#snippet bare()}
  <div class="mb-4 flex flex-wrap items-center justify-between gap-2">
    <h1 class="font-sans text-[20px] font-semibold leading-7 tracking-[-0.01em] text-fg">Today</h1>
    <div class="flex items-center gap-4">{@render tools()}</div>
  </div>
{/snippet}

<div class="relative flex h-full min-h-0">
  <div class={cn('min-w-0 flex-1 overflow-y-auto px-5 pb-16 pt-4 sm:px-8', drawerCard && drawerFull && 'hidden')}>
    <main use:staggerIn data-inbox-surface class="mx-auto w-full max-w-[var(--page-width)] space-y-3">
      {#if failed}
        <div>
          {@render bare()}
          <QueryError error={query.error} title="Your brief could not be loaded" onRetry={() => void query.refetch()} />
        </div>
      {:else}
        <div>
          {#if stale}
            <QueryError
              variant="inline"
              class="mb-4 rounded-lg border border-[color:var(--theme-danger)]/40 bg-[color:var(--theme-danger)]/5 px-4 py-2.5"
              error={query.error}
              title="This brief may be out of date; the last refresh failed"
              onRetry={() => void query.refetch()}
            />
          {/if}
          <!-- `query.data === undefined`, not `isLoading`: svelte-query reports
               "first fetch in flight" as false in the states between (a retry
               backoff, a mount before the fetch starts), and every one of those
               has no data and no error. -->
          <Materialize loading={query.data === undefined} count={1}>
            {#snippet skeleton()}<BriefSkeleton />{/snippet}

            {#if absent}
              {@render bare()}
              <div class="overflow-hidden rounded-xl border border-line bg-panel">
                <BriefAbsentState state={absent} />
              </div>
            {:else if brief}
              <InboxToday
                {brief}
                needs={board.lanes.needs}
                tally={laneTally(board.lanes, { withZeros: calm })}
                now={board.now}
                {calm}
                {tools}
                onRing={(glyph) => void inbox?.ring(glyph)}
                onAgenda={() => (aside = 'agenda')}
              />
            {/if}
          </Materialize>
        </div>
      {/if}

      {#if brief && !failed}
        <InboxMessages {messages} now={board.now} {actions} />
      {/if}

      <InboxBoard
        bind:this={inbox}
        {board}
        selectedKey={drawerKey}
        beside={drawerBeside}
        onOpen={(card) => openDrawer(card.key)}
        onBoard={() => void closeDrawer()}
      />
    </main>
  </div>

  {#if aside === 'mail' || aside === 'agenda'}
    <aside transition:fly={PANEL_X} class="flex w-[380px] shrink-0 flex-col overflow-hidden border-l border-line bg-panel">
      <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">{aside === 'mail' ? 'Mail' : 'Agenda'}</span>
        <span class="ml-auto"></span>
        <IconButton size="sm" title="Close" onclick={() => (aside = null)}><X size={13} /></IconButton>
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        {#if aside === 'mail'}{@render mail()}{:else}{@render agenda()}{/if}
      </div>
    </aside>
  {:else if aside && brief}
    {@const day = brief}
    <aside transition:fly={PANEL_X} class="flex w-[380px] shrink-0 flex-col overflow-hidden border-l border-line bg-panel">
      <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">Timeline</span>
        <span class="ml-auto"></span>
        {#if day.artifactId}
          <IconButton size="sm" title="Open this brief in Files" onclick={() => void navigate('/artifacts', { search: { a: day.artifactId! } })}>
            <ExternalLink size={13} />
          </IconButton>
        {/if}
        <IconButton size="sm" title="Close" onclick={() => (aside = null)}><X size={13} /></IconButton>
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <BriefTimeline brief={day} />
      </div>
    </aside>
  {/if}

  {#if drawerCard}
    {@const line = drawerCard.line}
    <InboxDrawer
      card={drawerCard}
      now={board.now}
      full={drawerFull}
      {actions}
      onClose={() => void closeDrawer()}
      onGone={() => (drawerBeside = drawerCard !== null)}
      onToggleFull={() => (drawerFull = !drawerFull)}
      onAsk={line ? () => ask(line) : undefined}
    />
  {/if}
</div>
