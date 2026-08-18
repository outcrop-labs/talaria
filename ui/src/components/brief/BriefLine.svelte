<script lang="ts">
  import { ArrowUpRight, Check, MessageSquareText } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { cn } from '@/lib/cn'
  import { navigate } from '@/router'
  import BriefReply from './BriefReply.svelte'
  import type { BriefLine, CommsState } from './daily-brief.svelte'

  /**
   * One line of the brief, and the reason the surface exists.
   *
   * FOLLOWABLE IS THE WHOLE POINT. A brief that names a blocked ticket and
   * cannot open it is a newsletter — the reader has to go find it themselves,
   * which is exactly the work the brief was supposed to save. So the row IS the
   * link: the whole thing is one click to the ticket, the channel, the approval
   * or the calendar event, and `sourceHref` came from the same source function
   * that decided the item needed the owner in the first place.
   *
   * INTERNAL AND EXTERNAL ARE NOT THE SAME CLICK. An in-app href is a client
   * navigation; a calendar event's href leaves for Google. They are marked
   * differently because a person deserves to know a click is about to leave.
   *
   * A RESOLVED LINE STAYS ON THE PAGE, struck through. That is the append-only
   * contract made visible: what somebody read at 08:00 is still findable at
   * 18:00, and "I already dealt with that" is a state the document can show
   * rather than a row that silently vanished.
   */
  let {
    line,
    onAsk,
    comms,
    onDecideReply,
    onDelegate,
  }: {
    line: BriefLine
    /** Ask the assistant about this line specifically. */
    onAsk: (line: BriefLine) => void
    /** Live conversation state, when this line is a conversation. */
    comms?: CommsState
    onDecideReply?: (draftId: string, decision: 'approve' | 'reject') => Promise<{ ok: boolean; error?: string }>
    onDelegate?: (channelId: string, granted: boolean) => void
  } = $props()

  // The reply block renders only on an OPEN conversation. On a resolved one the
  // question has been answered, and offering to send a reply under it would be
  // an affordance for something that has already happened.
  const showReply = $derived(!!comms && !!onDecideReply && !!onDelegate && !line.resolved)

  const entry = $derived(line.current)
  const external = $derived(!!entry.sourceHref && /^https?:\/\//.test(entry.sourceHref))
  const followable = $derived(!!entry.sourceHref && entry.sourceHref !== '/')

  /** A REAL ANCHOR, STRETCHED OVER THE ROW — not a div with `role="link"`.
   *
   *  The row cannot BE an anchor because it contains a button (ask), and a
   *  button inside a link is invalid HTML that browsers resolve unpredictably.
   *  The house answer to that is the stretched link: the anchor is a
   *  positioned overlay covering the row, the controls sit above it in the
   *  stacking order, and the semantics are the ones a link should have —
   *  middle-click opens a tab, right-click offers "copy link address",
   *  keyboard focus works without a hand-rolled keydown handler.
   *
   *  Internal hrefs are intercepted so the client router handles them; the
   *  `href` is still present and still correct, which is what makes the
   *  browser affordances work. */
  function follow(e: MouseEvent): void {
    const href = entry.sourceHref
    if (!href || !followable || external) return
    // Let the browser have the modified clicks — a new tab is a real intent.
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return
    e.preventDefault()
    void navigate(href as never)
  }
</script>

<div class={cn('rounded-lg', line.resolved && 'opacity-55')}>
<div
  class={cn(
    'group relative flex gap-3 rounded-lg border border-transparent px-3 py-2.5 transition-colors',
    followable && 'hover:border-line hover:bg-raised focus-within:border-line',
  )}
>
  {#if followable && entry.sourceHref}
    <!-- The stretched link. Covers the row, sits BELOW the controls, and is the
         only focusable thing in the row by default. `aria-label` carries the
         title because the anchor itself has no text of its own. -->
    <a
      href={entry.sourceHref}
      target={external ? '_blank' : undefined}
      rel={external ? 'noopener noreferrer' : undefined}
      aria-label={entry.title}
      onclick={follow}
      class="absolute inset-0 z-0 rounded-lg focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-[color:var(--theme-accent)]"
    ></a>
  {/if}

  <!-- The state marker. A dot for live, a tick for resolved — NOT a checkbox:
       nothing on this surface is completed by clicking it, and drawing a
       control that does not act is the fabricated-affordance rule. -->
  <div class="pointer-events-none relative mt-1 flex w-3 shrink-0 justify-center">
    {#if line.resolved}
      <Check size={12} class="text-[color:var(--theme-success)]" />
    {:else if line.unseen}
      <span class="mt-0.5 size-1.5 rounded-full bg-accent" title="New since you last looked"></span>
    {:else}
      <span class="mt-0.5 size-1.5 rounded-full bg-line-strong"></span>
    {/if}
  </div>

  <div class="pointer-events-none relative min-w-0 flex-1">
    <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
      <span class={cn('font-sans text-[13.5px] font-medium leading-5 text-fg', line.resolved && 'line-through')}>
        {entry.title}
      </span>
      {#if entry.badge}
        <Chip tone={entry.badge.tone}>{entry.badge.label}</Chip>
      {/if}
      {#if entry.statusLabel}
        <Chip>{entry.statusLabel}</Chip>
      {/if}
    </div>

    {#if entry.body && !line.resolved}
      <p class="mt-1 font-sans text-[12.5px] leading-5 text-muted">{entry.body}</p>
    {/if}

    <!-- The trail. Only shown once a line has actually moved, because on a line
         that has not, "1 update" is a control that says nothing. -->
    {#if line.history.length > 1}
      <div class="mt-1.5 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">
        {line.history.length} updates today
      </div>
    {/if}
  </div>

  <!-- Above the stretched link (`z-10`), so a click on Ask does not also
       navigate. Revealed on hover, and on focus so it is reachable by keyboard. -->
  <div class="relative z-10 flex shrink-0 items-start gap-0.5 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
    <IconButton size="sm" title="Ask your assistant about this" onclick={() => onAsk(line)}>
      <MessageSquareText size={13} />
    </IconButton>
    {#if followable}
      <span class="grid size-6 place-items-center text-ink-dim" title={external ? 'Opens outside Talaria' : 'Open'}>
        <ArrowUpRight size={13} />
      </span>
    {/if}
  </div>
</div>

  <!-- OUTSIDE the row above, deliberately. The row is one stretched anchor, so
       a draft rendered inside it would put Send and Discard under a link and
       every click on them would also navigate away. -->
  {#if showReply && comms}
    <div class="pl-6 pr-3">
      <BriefReply
        {comms}
        peer={line.current.title.replace(/ is waiting on you$/, '')}
        onDecide={onDecideReply!}
        onDelegate={onDelegate!}
      />
    </div>
  {/if}
</div>
