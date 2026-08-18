<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Activity, CalendarDays, ExternalLink, Mail, MessageSquareText, X } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import { fly, PANEL_X, staggerIn } from '@/lib/motion'
  import { cn } from '@/lib/cn'
  import { navigate } from '@/router'
  import BriefAbsentState from './BriefAbsent.svelte'
  import BriefChat from './BriefChat.svelte'
  import BriefHero from './BriefHero.svelte'
  import BriefLineRow from './BriefLine.svelte'
  import BriefSkeleton from './BriefSkeleton.svelte'
  import BriefTimeline from './BriefTimeline.svelte'
  import HomeTabs from '@/routes/app/home/HomeTabs.svelte'
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
   * already there. What is left for a panel is the two things reading a
   * document makes you want: how did today move, and let me ask about this.
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

  type Aside = 'timeline' | 'chat' | 'mail' | 'agenda'
  let aside = $state<Aside | null>(null)
  let focus = $state<BriefLine | null>(null)

  const brief = $derived(query.data && !isBriefAbsent(query.data) ? (query.data as BriefView) : null)
  const absent = $derived(query.data && isBriefAbsent(query.data) ? query.data : null)
  const failed = $derived(query.isError && query.data === undefined)
  const stale = $derived(query.isError && query.data !== undefined)

  function ask(line: BriefLine): void {
    focus = line
    aside = 'chat'
  }
</script>

<div class="flex h-full min-h-0">
  <div class="min-w-0 flex-1 overflow-y-auto px-4 pb-16 pt-8 sm:px-8 sm:pt-12">
    <main use:staggerIn class="mx-auto w-full max-w-[800px]">
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
          title="This brief may be out of date — the last refresh failed"
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
                {day.unseenCount > 0 ? `${day.unseenCount} new since you looked` : 'How today moved'}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onclick={() => {
                  focus = null
                  aside = aside === 'chat' ? null : 'chat'
                }}
              >
                <MessageSquareText size={13} /> Ask {day.agent.name ?? 'your assistant'}
              </Button>
              {#if day.artifactId}
                <!-- The mirror. Offered rather than pushed: the document on
                     screen is the brief, and this is where you go to share or
                     export it. -->
                <Button size="sm" variant="ghost" onclick={() => void navigate('/artifacts', { search: { a: day.artifactId! } })}>
                  <ExternalLink size={13} /> Open as artifact
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
                        <BriefLineRow {line} onAsk={ask} />
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
    <!-- A peer of the document, not a modal over it: the point of both panels
         is to be read NEXT TO the line they are about. -->
    <aside
      transition:fly={PANEL_X}
      class="flex w-[380px] shrink-0 flex-col overflow-hidden border-l border-line bg-panel"
    >
      <header class="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
          {aside === 'timeline' ? 'How today moved' : `Ask ${day.agent.name ?? 'your assistant'}`}
        </span>
        <span class="ml-auto"></span>
        <IconButton size="sm" title="Close" onclick={() => (aside = null)}>
          <X size={13} />
        </IconButton>
      </header>
      <div class={cn('min-h-0 flex-1 overflow-y-auto p-3', aside === 'chat' && 'flex flex-col overflow-hidden')}>
        {#if aside === 'timeline'}
          <BriefTimeline brief={day} />
        {:else}
          <BriefChat brief={day} {focus} onClearFocus={() => (focus = null)} />
        {/if}
      </div>
    </aside>
  {/if}
</div>
