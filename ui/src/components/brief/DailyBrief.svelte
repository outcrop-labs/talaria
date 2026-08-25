<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Activity, CalendarDays, ExternalLink, Mail, X } from '@lucide/svelte'
  import { getContext } from 'svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { fly, PANEL_X, staggerIn } from '@/lib/motion'
  import { navigate } from '@/router'
  import BriefAbsentState from './BriefAbsent.svelte'
  import BriefHero from './BriefHero.svelte'
  import BriefLineRow from './BriefLine.svelte'
  import BriefSkeleton from './BriefSkeleton.svelte'
  import BriefTimeline from './BriefTimeline.svelte'
  import { INBOX_FOCUS_WORKSPACE_KEY, type InboxFocusWorkspaceValue } from '@/components/inbox/inbox-focus-shell'
  import HomeTabs from '@/routes/app/home/HomeTabs.svelte'
  import { greeting } from '@/routes/app/home/home'
  import { useSession } from '@/lib/session'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import {
    isBriefAbsent,
    SECTION_HINT,
    SECTION_TITLE,
    useBrief,
    useBriefActions,
    useBriefLive,
    type BriefLine,
    type BriefView,
  } from './daily-brief.svelte'

  /**
   * The daily brief — the home surface.
   *
   * WHAT REPLACED WHAT. This stands where the focus queue stood, and the swap
   * is not cosmetic. A queue answers "what is the single next decision", which
   * is the right question once a day and the wrong one every other time the app
   * is opened; it reorders itself under the reader, so it cannot answer "what
   * changed while I was away" at any length. This is a document that is written
   * once and appended to, so it answers both by construction.
   *
   * THE DOCUMENT IS THE FOCAL POINT and the aside is chrome. In the source
   * sketch the brief was an artifact that opened in a side panel next to a
   * chat; here the relationship is inverted, because the chat was only ever the
   * way to REQUEST the brief and nobody needs to request a thing that is
   * already there. What is left for a panel is how today moved.
   *
   * ASKING GOES TO THE SIDEBAR, not to a chat panel of the brief's own. The
   * brief used to carry its own right-hand conversation (one thread per line);
   * that is gone — a second chat beside the assistant the sidebar already
   * launches is a second thing to find, and the line's key resolves inside the
   * sidebar conversation exactly as it did in the private one. `ask` below
   * hands the line to the shell, which opens the panel with it attached.
   *
   * FAILURE MODES, and they are the same two `FocusInbox` was split for — this
   * is the surface everything blocked on a person arrives at, so the one thing
   * it must never do is render calm over a read it could not make:
   *   · a failed BACKGROUND refetch keeps the document and says it is stale.
   *     Stale beats blank, as long as it says it is stale.
   *   · a failed FIRST read renders the error, never an empty day.
   */
  // Mail and Agenda came across from the focus queue unchanged. They are
  // Google reads that live on Home and are NOT part of the brief: the brief's
  // schedule section is written once in the morning and appended to, whereas
  // these are live windows onto an external system. Folding a live pane into an
  // append-only document would have been the first crack in the one rule this
  // feature has.
  let { mail, agenda }: { mail: Snippet; agenda: Snippet } = $props()

  const query = useBrief()
  const actions = useBriefActions()
  useBriefLive()
  const session = useSession()

  // The strip title is the SAME greeting ConsoleHome claims, for the same
  // reason the in-body header used to match: Inbox is a Home tab, and a
  // different title on the tab people land on would swap the strip's words on
  // every move between it and a sibling. Matching is what makes it hold still.
  claimViewTitle(greeting(session.data?.name ?? session.data?.email))
  // May be absent only in tests; the layout always provides the shell.
  const workspace = getContext<InboxFocusWorkspaceValue | undefined>(INBOX_FOCUS_WORKSPACE_KEY)

  type Aside = 'timeline' | 'mail' | 'agenda'
  let aside = $state<Aside | null>(null)

  const brief = $derived(query.data && !isBriefAbsent(query.data) ? (query.data as BriefView) : null)
  const absent = $derived(query.data && isBriefAbsent(query.data) ? query.data : null)
  const failed = $derived(query.isError && query.data === undefined)
  const stale = $derived(query.isError && query.data !== undefined)

  /** The line's key rides the sidebar command only when the focus world can
   *  resolve it — task/notification/channel/approval keys ARE focus keys (the
   *  brief's sources are the focus sources). A calendar line has no focus
   *  counterpart, so asking about one opens the panel without a key: a general
   *  question, honestly, rather than a "that item changed" bounce off a key
   *  the server has never known. */
  const FOCUS_SOURCE_TYPES = new Set(['task', 'notification', 'channel', 'approval'])

  function ask(line: BriefLine): void {
    const key = line.current.sourceType && FOCUS_SOURCE_TYPES.has(line.current.sourceType) ? line.key : null
    workspace?.askAbout({ key, question: line.current.title })
  }
</script>

<div class="flex h-full min-h-0">
  <div class="min-w-0 flex-1 overflow-y-auto p-8 pb-16">
    <main use:staggerIn class="mx-auto w-full max-w-[var(--page-width)]">
      <!-- Home's tab strip. It lived in `FocusInbox` for the same reason it
           lives here: `ConsoleHome` never renders for this tab, so without it
           the surface most people land on has no way out of itself. -->
      <div class="mb-6 flex flex-wrap items-center gap-2">
        <div class="min-w-0 flex-1"><HomeTabs value="inbox" /></div>
        <Button variant="outline" size="xs" class="h-8 gap-1.5 px-2.5 text-muted hover:text-fg" onclick={() => (aside = aside === 'mail' ? null : 'mail')}>
          <Mail size={12} /> Mail
        </Button>
        <Button variant="outline" size="xs" class="h-8 gap-1.5 px-2.5 text-muted hover:text-fg" onclick={() => (aside = aside === 'agenda' ? null : 'agenda')}>
          <CalendarDays size={12} /> Agenda
        </Button>
      </div>

      {#if stale}
        <QueryError
          variant="inline"
          class="mb-5 rounded-lg border border-[color:var(--theme-danger)]/40 bg-[color:var(--theme-danger)]/5 px-4 py-2.5"
          error={query.error}
          title="This brief may be out of date; the last refresh failed"
          onRetry={() => void query.refetch()}
        />
      {/if}

      {#if failed}
        <QueryError error={query.error} title="Your brief could not be loaded" onRetry={() => void query.refetch()} />
      {:else}
        <!-- `query.data === undefined`, not `isLoading`: svelte-query reports
             "first fetch in flight" as false in the states between (a retry
             backoff, a mount before the fetch starts), and every one of those
             has no data and no error. Branching on `isLoading` would drop them
             through to the empty state — "nothing is waiting on you", asserted
             by a surface that has not asked yet. -->
        <Materialize loading={query.data === undefined} count={1}>
          {#snippet skeleton()}<BriefSkeleton />{/snippet}

          {#if absent}
            <BriefAbsentState state={absent} />
          {:else if brief}
            {@const day = brief}
            <BriefHero brief={day} />

            <!-- The two ways into the aside, and the counter that earns the
                 first one. `unseenCount` comes from the server's read cursor,
                 so it survives a reload and does not clear just because the
                 page rendered. -->
            <div class="mt-4 flex flex-wrap items-center gap-2">
              <Button
                size="sm"
                variant={day.unseenCount > 0 ? 'outline' : 'ghost'}
                onclick={() => {
                  aside = aside === 'timeline' ? null : 'timeline'
                  if (aside === 'timeline') actions.markRead(day)
                }}
              >
                <Activity size={13} />
                {day.unseenCount > 0 ? `${day.unseenCount} new since you looked` : 'Timeline'}
              </Button>
              {#if day.artifactId}
                <!-- The mirror. Offered rather than pushed: the document on
                     screen is the brief, and this is where you go to share or
                     export it.
                     "Files", not "artifact": that view is called Files in the
                     nav (lib/nav.ts), and naming the same destination twice is
                     how a person ends up looking for a section that isn't
                     there. `artifact` is the word the schema uses, not the word
                     on the screen. -->
                <Button size="sm" variant="ghost" onclick={() => void navigate('/artifacts', { search: { a: day.artifactId! } })}>
                  <ExternalLink size={13} /> Open in Files
                </Button>
              {/if}
            </div>

            {#if day.sections.length === 0}
              <!-- Reachable only with data in hand, so "nothing is waiting" is
                   something the server actually said. -->
              <div class="mt-10 rounded-lg border border-line px-6 py-10 text-center">
                <p class="font-sans text-sm text-muted">
                  Nothing is waiting on you right now. Anything that changes today gets appended here.
                </p>
              </div>
            {:else}
              <div class="mt-10 space-y-10">
                {#each day.sections as section (section.section)}
                  <section>
                    <SectionHeader
                      title={SECTION_TITLE[section.section] ?? section.section}
                      info={SECTION_HINT[section.section]}
                      action={`${String(section.lines.filter((l) => !l.resolved).length).padStart(2, '0')} OPEN`}
                    />
                    <div class="-mx-3">
                      {#each section.lines as line (line.key)}
                        <BriefLineRow
                          {line}
                          onAsk={ask}
                          comms={day.comms.find((c) => c.sourceKey === line.key)}
                          onDecideReply={actions.decideReply}
                          onDelegate={(channelId, granted) => void actions.setDelegated(channelId, granted)}
                          onMark={actions.markItem}
                        />
                      {/each}
                    </div>
                  </section>
                {/each}
              </div>
            {/if}
          {/if}
        </Materialize>
      {/if}
    </main>
  </div>

  {#if aside === 'mail' || aside === 'agenda'}
    <aside transition:fly={PANEL_X} class="flex w-[380px] shrink-0 flex-col overflow-hidden border-l border-line bg-panel">
      <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{aside === 'mail' ? 'Mail' : 'Agenda'}</span>
        <span class="ml-auto"></span>
        <IconButton size="sm" title="Close" onclick={() => (aside = null)}><X size={13} /></IconButton>
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        {#if aside === 'mail'}{@render mail()}{:else}{@render agenda()}{/if}
      </div>
    </aside>
  {:else if aside && brief}
    {@const day = brief}
    <!-- A peer of the document, not a modal over it: the timeline is read NEXT
         TO the lines it is about. The panel that used to sit here as a second
         slot — the brief's private chat — is gone; asking goes to the sidebar
         assistant (see `ask` in the script), which carries the conversation
         the sidebar already owns. -->
    <aside
      transition:fly={PANEL_X}
      class="flex w-[380px] shrink-0 flex-col overflow-hidden border-l border-line bg-panel"
    >
      <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Timeline</span>
        <span class="ml-auto"></span>
        <IconButton size="sm" title="Close" onclick={() => (aside = null)}>
          <X size={13} />
        </IconButton>
      </header>
      <div class="min-h-0 flex-1 overflow-y-auto p-3">
        <BriefTimeline brief={day} />
      </div>
    </aside>
  {/if}
</div>
