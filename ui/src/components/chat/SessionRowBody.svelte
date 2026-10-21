<script lang="ts">
  import { Pencil, Pin } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { WaitingSiteKey } from '@/lib/waiting/sites'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import CountPill from '@/components/app/CountPill.svelte'
  import type { Conversation } from '@/lib/conversations.svelte'

  /** §10 session row content: [status dot] [13px title] [mono owner] [mono time]
   *  [unread pill]. Shared: the Plan rail and the Comms agent-thread rows are
   *  both session lists — one anatomy, one component.
   *
   *  A row can also carry state and a kind that are NOT the conversation's —
   *  a research run's queued/running/done, the mode it ran in. Those arrive as
   *  `status`/`pulse`/`chip` rather than as more fields on Conversation: the
   *  anatomy is the same row either way, and this component has no business
   *  knowing what a run is. */
  let {
    conv,
    active,
    metaClass,
    showUnread = true,
    draft = false,
    pinned = false,
    waitingSite,
    status,
    pulse,
    chip,
  }: {
    conv: Conversation
    active: boolean
    /** Extra classes for the trailing meta (pill + time) — the Comms rail
     *  hides it on hover, where the row's actions take its place. */
    metaClass?: string
    /** The unread pill. The Comms rail drops it — a bold title says unread. */
    showUnread?: boolean
    /** A half-written message waits in this session's composer (the Comms
     *  rail's pencil). */
    draft?: boolean
    /** The viewer pinned this thread — the glyph that says why the row is at
     *  the top of the rail. A PROP rather than `conv.pinned`, because the pin
     *  is only an explanation where the surface actually leads with pinned
     *  rows; on a list that does not sort by it, a pin glyph is a claim about
     *  an order nobody can see. */
    pinned?: boolean
    /** While the session's agent is writing, show the waiting mark (this
     *  site) beside the title instead of the time. */
    waitingSite?: WaitingSiteKey
    /** State of the row's OWN, in place of the session dot — a research run is
     *  queued or errored whatever its conversation is doing. */
    status?: DotStatus
    /** Breathe the dot above while that state is actually live. */
    pulse?: boolean
    /** A kind, in the meta line's mono chrome voice ("DEEP", "BRIEF"). */
    chip?: string
  } = $props()

  // Semantic first (spec §1): failure orange, working green; the active row's
  // dot reads gold, idle rows stay dim ink.
  const dot = $derived(
    conv.failed
      ? 'var(--theme-danger)'
      : conv.working
        ? 'var(--theme-success)'
        : active
          ? 'var(--theme-accent)'
          : 'var(--theme-ink-dim)',
  )
</script>

{#if status}
  <!-- ONE DOT BEFORE THE TITLE, NEVER TWO: a row has one state, so a status of
       the row's own replaces the session dot rather than sitting next to it.
       Same 6px geometry — StatusDot's default is this dot. -->
  <StatusDot {status} {pulse} />
{:else}
  <span
    aria-hidden="true"
    class={cn('h-[6px] w-[6px] shrink-0 rounded-full', conv.working && !conv.failed && 'gd-breathe')}
    style:background-color={dot}
  ></span>
  <!-- A working session is live background state — MONITOR BREATHE on the
       ambient budget (spec §9); idle/failed dots stay still. -->
{/if}
<span class="min-w-0 flex-1 truncate font-sans text-[13px]">{conv.title || 'Untitled'}</span>
{#if conv.ownerLabel}
  <span class="max-w-16 shrink-0 truncate font-mono text-[10px] tracking-[0.05em] text-muted">
    {conv.ownerLabel}
  </span>
{/if}
<!-- The unread pill sits before the time: what is waiting outranks when it
     waited. Renders nothing at zero — a read thread carries no chrome. -->
<span class={cn('flex shrink-0 items-center gap-1.5', metaClass)}>
  <!-- The kind leads the meta line: what the row IS outranks what is waiting
       on it, and both outrank when it last moved. -->
  {#if chip}<Chip class="max-w-24 truncate">{chip}</Chip>{/if}
  {#if pinned}<Pin size={10} class="shrink-0 text-muted" aria-label="Pinned" />{/if}
  {#if draft}<Pencil size={11} class="shrink-0 text-muted" aria-label="Draft" />{/if}
  {#if showUnread}<CountPill count={conv.unreadCount} />{/if}
  {#if waitingSite && conv.working && !conv.failed}
    <!-- The agent is writing: the waiting mark plays where the time sits, and
         the time comes back when the reply lands. -->
    <WaitingMark site={waitingSite} size={12} class="shrink-0 text-accent" />
  {:else}
    <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-ink-dim">
      {relativeTime(conv.updatedAt)}
    </span>
  {/if}
</span>
