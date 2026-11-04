<script lang="ts">
  import { untrack, type Component } from 'svelte'
  import { MediaQuery } from 'svelte/reactivity'
  import {
    FileText,
    FlaskConical,
    GitBranch,
    GitPullRequest,
    Globe,
    MessagesSquare,
    Pencil,
    RotateCcw,
    Search,
    SquareTerminal,
    Wrench,
  } from '@lucide/svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import {
    clock,
    LANE,
    layout,
    livePath,
    MAX_ROWS,
    PAD,
    path,
    plural,
    RAIL,
    rewindStageWords,
    ROW,
    summarize,
    TOUCH_ROW,
    x,
    type Checkpoint,
    type Placed,
    type ToolKind,
  } from '@/lib/agent-checkpoints'
  import { prefersReducedMotion } from '@/lib/motion'

  let {
    checkpoints,
    live = false,
    rewinding = false,
    onRewind,
    focusAttempt = null,
    onFocused,
  }: {
    checkpoints: Checkpoint[]
    live?: boolean
    /** A rewind of this lineage is under way, so no row offers another. */
    rewinding?: boolean
    onRewind?: (target: { runId: string; seq: number }) => void
    /** The run whose opening row takes focus once it is drawn. */
    focusAttempt?: string | null
    onFocused?: () => void
  } = $props()

  const TOOLS: Record<ToolKind, Component> = {
    turn: MessagesSquare,
    search: Search,
    read: FileText,
    edit: Pencil,
    shell: SquareTerminal,
    test: FlaskConical,
    browser: Globe,
    git: GitPullRequest,
    other: Wrench,
  }

  const MAX_GUTTER_SHARE = 1 / 3
  const GUTTER_INSET = RAIL + 16

  const uid = $props.id()

  let list = $state<HTMLDivElement>()
  let width = $state(0)
  let reasonsOpen = $state<string[]>([])
  let measured = $state<Record<string, number>>({})

  const touch = new MediaQuery('(hover: none), (max-width: 480px)')
  const rowHeight = $derived(touch.current ? TOUCH_ROW : ROW)

  type Note = { tone: 'reason' | 'failed'; text: string }

  function noteOf(cp: Checkpoint): Note | undefined {
    if (cp.rewind?.kind === 'failed') return { tone: 'failed', text: cp.rewind.reason }
    if (cp.answer?.kind === 'reason' && reasonsOpen.includes(cp.id)) return { tone: 'reason', text: cp.answer.reason }
  }

  const notes = $derived(
    Object.fromEntries(checkpoints.filter((cp) => noteOf(cp)).map((cp) => [cp.id, measured[cp.id] ?? 0])),
  )
  const placed = $derived(layout(checkpoints, { row: rowHeight, notes }))
  const total = $derived(placed.reduce((end, p) => Math.max(end, p.top + rowHeight + (notes[p.cp.id] ?? 0)), 0))
  const sum = $derived(summarize(placed))
  const failedRewinds = $derived(checkpoints.filter((cp) => cp.rewind?.kind === 'failed').length)
  const headline = $derived(
    [
      plural(sum.checkpoints, 'checkpoint'),
      sum.attempts > 1 && plural(sum.attempts, 'attempt'),
      rewinding && 'Rewinding',
      failedRewinds > 0 && `${plural(failedRewinds, 'rewind')} failed`,
    ]
      .filter(Boolean)
      .join(' · '),
  )
  const pitch = $derived(
    sum.lanes > 1 && width ? Math.min(LANE, (width * MAX_GUTTER_SHARE - GUTTER_INSET) / (sum.lanes - 1)) : LANE,
  )
  const head = $derived(placed.at(-1))
  const onPath = $derived(livePath(placed))
  const scrolls = $derived(total > MAX_ROWS * rowHeight)

  const idsAtOpen = untrack(() => new Set(checkpoints.map((cp) => cp.id)))
  const isNew = (cp: Checkpoint) => !idsAtOpen.has(cp.id)

  const hasLine = (p: Placed): p is Placed & { fromLane: number; fromMid: number } =>
    p.fromLane !== undefined && p.fromMid !== undefined

  const canRewind = (cp: Checkpoint) => !!onRewind && !rewinding && cp.answer?.kind === 'rewind'

  function answer(cp: Checkpoint) {
    if (cp.answer?.kind === 'reason') {
      reasonsOpen = reasonsOpen.includes(cp.id) ? reasonsOpen.filter((id) => id !== cp.id) : [...reasonsOpen, cp.id]
    } else if (cp.answer && canRewind(cp)) {
      onRewind?.(cp.answer)
    }
  }

  function rowLabel(p: Placed, isHead: boolean) {
    const { cp } = p
    return [
      cp.title,
      p.attempt && `attempt ${p.attempt}`,
      cp.status !== 'done' && cp.status,
      isHead ? 'current' : !onPath.has(p.row) && 'left behind',
      cp.rewind?.kind === 'pending' && `rewinding: ${rewindStageWords(cp.rewind.stage)}`,
      cp.rewind?.kind === 'failed' && 'rewind failed',
      canRewind(cp) && 'rewind to this step',
      cp.answer?.kind === 'reason' && 'cannot be rewound, show why',
    ]
      .filter(Boolean)
      .join(', ')
  }

  let following = true
  let landed = false
  let lastTop = 0

  // A smooth scroll to the newest row passes through "not at the end" on its
  // way there, so only an upward move means the reader left.
  function onScroll() {
    if (!list) return
    const top = list.scrollTop
    if (top + list.clientHeight >= list.scrollHeight - 2) following = true
    else if (top < lastTop) following = false
    lastTop = top
  }

  $effect(() => {
    const height = total
    if (!list || !scrolls || !following) return
    const smooth = landed && !prefersReducedMotion()
    landed = true
    list.scrollTo({ top: height + PAD, behavior: smooth ? 'smooth' : 'auto' })
  })

  $effect(() => {
    if (!focusAttempt || !placed.length) return
    const row = list?.querySelector<HTMLElement>(`[data-opens="${focusAttempt}"]`)
    if (!row) return
    row.focus()
    onFocused?.()
  })
</script>

<section
  class="ackc rounded-lg border border-line-subtle"
  aria-labelledby="{uid}-title"
  bind:clientWidth={width}
  data-touch={touch.current ? '' : undefined}
  style:--ackc-row="{rowHeight}px"
  style:--ackc-pad="{PAD}px"
  style:--ackc-max-rows={MAX_ROWS}
  style:--ackc-gutter="{GUTTER_INSET + (sum.lanes - 1) * pitch}px"
>
  <header class="head">
    <GitBranch class="h-3.5 w-3.5 shrink-0 text-muted" aria-hidden="true" />
    <h2 id="{uid}-title" class="title">Checkpoints</h2>
    {#if placed.length > 0}<span class="sum" aria-live="polite">{headline}</span>{/if}
  </header>

  {#if placed.length === 0}
    {#if live}
      <EmptyState variant="compact" title="No checkpoints yet" hint="They appear here as the agent works." />
    {:else}
      <EmptyState
        variant="compact"
        title="No checkpoints were kept for this run"
        hint="It ended before its first checkpoint, or before Talaria kept them."
      />
    {/if}
  {:else}
    <div
      bind:this={list}
      class="list"
      style:--ackc-total="{total}px"
      data-scrolls={scrolls ? '' : undefined}
      onscroll={onScroll}
    >
      <svg class="rail" width={x(sum.lanes, pitch)} height={total} aria-hidden="true">
        <defs>
          <linearGradient id="{uid}-glint" x1="0" x2="1" y1="0" y2="0">
            <stop class="glint-stop" offset="0" stop-opacity="0" />
            <stop class="glint-stop" offset="0.5" stop-opacity="0.9" />
            <stop class="glint-stop" offset="1" stop-opacity="0" />
          </linearGradient>
          {#each Object.entries(TOOLS) as [kind, Mark] (kind)}
            <Mark id="{uid}-{kind}" />
          {/each}
        </defs>
        {#each placed as p (p.cp.id)}
          {#if hasLine(p)}
            <path
              class="line"
              d={path(p, pitch, rowHeight)}
              pathLength={1}
              data-path={onPath.has(p.row) ? '' : undefined}
              data-fork={p.lane !== p.fromLane ? '' : undefined}
              data-new={isNew(p.cp) ? '' : undefined}
            />
          {/if}
        {/each}
        {#each placed as p (p.cp.id)}
          {@const isHead = p.cp.id === head?.cp.id}
          <g
            class="dot"
            data-path={onPath.has(p.row) ? '' : undefined}
            data-head={isHead ? '' : undefined}
            data-status={p.cp.status}
            data-fork={p.lane !== p.fromLane ? '' : undefined}
            data-new={isNew(p.cp) ? '' : undefined}
          >
            <circle cx={x(p.lane, pitch)} cy={p.mid} r={isHead ? 4.5 : 3.5} />
            {#if isHead}
              <clipPath id="{uid}-clip-{p.row}">
                <circle cx={x(p.lane, pitch)} cy={p.mid} r={4.5} />
              </clipPath>
              <g clip-path="url(#{uid}-clip-{p.row})">
                <rect
                  class="glint"
                  x={x(p.lane, pitch) - 4}
                  y={p.mid - 8}
                  width={8}
                  height={16}
                  fill="url(#{uid}-glint)"
                />
              </g>
            {/if}
          </g>
        {/each}
      </svg>

      <ol>
        {#each placed as p (p.cp.id)}
          {@const isHead = p.cp.id === head?.cp.id}
          {@const note = noteOf(p.cp)}
          {@const pending = p.cp.rewind?.kind === 'pending' ? p.cp.rewind : undefined}
          <li class="slot" data-new={isNew(p.cp) ? '' : undefined} data-fork={p.lane !== p.fromLane ? '' : undefined}>
            <button
              type="button"
              class="row"
              data-path={onPath.has(p.row) ? '' : undefined}
              data-status={p.cp.status}
              data-working={p.cp.status === 'running' ? '' : undefined}
              data-rewindable={canRewind(p.cp) ? '' : undefined}
              data-opens={p.attempt ? p.cp.runId : undefined}
              aria-current={isHead ? 'step' : undefined}
              aria-label={rowLabel(p, isHead)}
              aria-disabled={p.cp.answer?.kind === 'rewind' && !canRewind(p.cp) ? 'true' : undefined}
              aria-expanded={p.cp.answer?.kind === 'reason' ? note !== undefined : undefined}
              aria-describedby={note ? `${uid}-note-${p.row}` : undefined}
              onclick={() => answer(p.cp)}
            >
              <span class="tool" data-kind={p.cp.tool}>
                <svg width="11" height="11" aria-hidden="true"><use href="#{uid}-{p.cp.tool}" width="11" height="11" /></svg>
              </span>
              <span class="name" title={p.cp.title}>{p.cp.title}</span>
              {#if p.attempt}<Chip>Attempt {p.attempt}</Chip>{/if}
              {#if p.cp.status === 'failed'}<Chip tone="danger">Failed</Chip>{/if}
              {#if p.cp.status === 'stopped'}<Chip>Stopped</Chip>{/if}
              {#if pending}<Chip tone="accent">{rewindStageWords(pending.stage)}</Chip>{/if}
              {#if p.cp.rewind?.kind === 'failed'}<Chip tone="danger">Rewind failed</Chip>{/if}
              <span class="time">{clock(p.cp.at)}</span>
              {#if canRewind(p.cp)}
                <span class="act" aria-hidden="true"><RotateCcw class="h-3 w-3" /><span class="word">Rewind</span></span>
              {:else if p.cp.answer?.kind === 'reason'}
                <span class="act" data-quiet aria-hidden="true">No rewind</span>
              {/if}
            </button>
            {#if note}
              <p
                id="{uid}-note-{p.row}"
                class="note"
                data-tone={note.tone}
                bind:clientHeight={measured[p.cp.id]}
              >
                {note.text}
              </p>
            {/if}
          </li>
        {/each}
      </ol>
    </div>
  {/if}
</section>

<style>
  .ackc {
    --ackc-path: color-mix(in srgb, var(--theme-accent) 70%, var(--theme-panel));
    --ackc-rail: var(--theme-border-strong);
    --ackc-edge: 14px;
    --ackc-ease: cubic-bezier(0.32, 0.72, 0.24, 1);
    --ackc-fade: 320ms;
    container: ackc / inline-size;
    font-size: 13px;
    line-height: 20px;
  }

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 8px;
    padding: 12px var(--ackc-edge) 6px 15px;
  }
  .title {
    flex: none;
    font-family: var(--font-sans);
    font-weight: 500;
    color: var(--theme-text);
    white-space: nowrap;
  }
  .sum {
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 16px;
    font-variant-numeric: tabular-nums;
    color: var(--theme-muted);
    white-space: nowrap;
  }

  .list {
    position: relative;
    isolation: isolate;
    height: calc(var(--ackc-total) + var(--ackc-pad));
    max-height: calc(var(--ackc-max-rows) * var(--ackc-row) + var(--ackc-pad));
    overflow: hidden;
    overscroll-behavior: contain;
    scrollbar-width: thin;
    transition: height 220ms var(--ackc-ease);
  }
  .list[data-scrolls] {
    overflow-y: auto;
  }
  ol {
    margin: 0;
    padding: 0 0 var(--ackc-pad);
    list-style: none;
  }

  .rail {
    position: absolute;
    top: 0;
    left: 0;
    z-index: 1;
    display: block;
    overflow: visible;
    pointer-events: none;
  }

  .line {
    fill: none;
    stroke: var(--ackc-rail);
    stroke-width: 1.5;
    stroke-linecap: round;
    stroke-dasharray: 1;
    stroke-dashoffset: 0;
    transition: stroke var(--ackc-fade) ease-out;
  }
  .line[data-path] {
    stroke: var(--ackc-path);
  }
  .line[data-new] {
    animation: ackc-draw 220ms var(--ackc-ease) both;
  }
  .line[data-new][data-fork] {
    animation-duration: 380ms;
  }

  .dot {
    transform-box: fill-box;
    transform-origin: center;
  }
  .dot[data-new] {
    animation: ackc-pop 180ms var(--ackc-ease) both;
  }
  .dot[data-new][data-fork] {
    animation-delay: 240ms;
  }
  .dot circle {
    fill: var(--theme-panel);
    stroke: var(--ackc-rail);
    stroke-width: 1.5;
    transition:
      fill var(--ackc-fade) ease-out,
      stroke var(--ackc-fade) ease-out;
  }
  .dot[data-path] circle {
    fill: var(--ackc-path);
    stroke: var(--theme-panel);
  }
  .dot[data-head] circle {
    fill: var(--theme-accent);
    stroke: var(--theme-panel);
  }
  .dot[data-status='failed'] circle {
    stroke: color-mix(in srgb, var(--theme-danger) 50%, var(--theme-panel));
  }
  .dot[data-path][data-status='failed'] circle {
    fill: var(--theme-danger);
    stroke: var(--theme-panel);
  }
  .glint-stop {
    stop-color: var(--theme-accent-secondary);
  }
  .glint {
    transform-box: fill-box;
    transform-origin: center;
    animation: ackc-glint 700ms 120ms ease-in-out both;
  }

  .slot {
    content-visibility: auto;
    contain-intrinsic-height: auto var(--ackc-row);
  }
  .slot[data-new] {
    animation: ackc-rise 180ms var(--ackc-ease) both;
  }
  .slot[data-new][data-fork] {
    animation-delay: 240ms;
  }

  .row {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: var(--ackc-row);
    padding: 0 var(--ackc-edge) 0 var(--ackc-gutter);
    border: 0;
    outline: none;
    background: none;
    font: inherit;
    color: inherit;
    text-align: left;
    cursor: default;
    touch-action: manipulation;
    -webkit-tap-highlight-color: transparent;
    transition: padding-left 240ms var(--ackc-ease);
  }
  .row[data-rewindable] {
    cursor: pointer;
  }
  .row::before {
    content: '';
    position: absolute;
    inset: 2px 6px;
    z-index: -1;
    border-radius: 6px;
    background: var(--theme-hover);
    opacity: 0;
    transition: opacity 150ms ease-out;
  }
  .row:focus-visible::before {
    opacity: 1;
    outline: 1.5px solid var(--theme-focus);
  }
  .row:active::before {
    opacity: 1;
  }

  .tool {
    display: grid;
    flex: none;
    place-items: center;
    width: 18px;
    height: 18px;
    border: 1px solid var(--theme-border);
    border-radius: 4px;
    background: var(--theme-raised);
    color: var(--theme-muted);
    transition:
      border-color var(--ackc-fade) ease-out,
      background-color var(--ackc-fade) ease-out;
  }
  .name {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-sans);
    color: var(--theme-text);
    white-space: nowrap;
    text-overflow: ellipsis;
    transition: color var(--ackc-fade) ease-out;
  }
  .row[data-working] .name {
    background: linear-gradient(
        90deg,
        var(--theme-text) 0 40%,
        var(--theme-ink-dim) 50%,
        var(--theme-text) 60% 100%
      )
      0 0 / 250% 100%;
    -webkit-background-clip: text;
    background-clip: text;
    color: transparent;
    animation: ackc-shine 800ms linear infinite;
  }
  .row[data-status='stopped'] .name {
    color: var(--theme-muted);
  }

  .row:not([data-path]) .name {
    color: var(--theme-muted);
  }
  .row:not([data-path]) .tool {
    border-color: var(--theme-border-subtle);
    background: none;
  }
  @media (hover: hover) and (pointer: fine) {
    .row:hover::before {
      opacity: 1;
    }
    .row:not([data-path]):hover .name,
    .row:hover .time {
      color: var(--theme-text);
    }
  }

  .time {
    flex: none;
    min-width: 30px;
    margin-left: auto;
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 16px;
    font-variant-numeric: tabular-nums;
    color: var(--theme-muted);
    text-align: right;
    white-space: nowrap;
    transition:
      color 150ms ease-out,
      opacity 150ms ease-out;
  }

  .act {
    position: absolute;
    top: 50%;
    right: 8px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    padding: 0 8px;
    border: 1px solid var(--theme-border-strong);
    border-radius: 6px;
    background: var(--theme-panel);
    font-family: var(--font-sans);
    font-size: 11px;
    line-height: 16px;
    color: var(--theme-text);
    white-space: nowrap;
    opacity: 0;
    pointer-events: none;
    transform: translateY(-50%) scale(0.96);
    transition:
      opacity 150ms ease-out,
      transform 150ms ease-out;
  }
  .act[data-quiet] {
    border-color: transparent;
    background: none;
    color: var(--theme-muted);
  }
  .row:focus-visible .act {
    opacity: 1;
    transform: translateY(-50%);
  }
  .row:focus-visible:has(.act) .time {
    opacity: 0;
  }
  @media (hover: hover) and (pointer: fine) {
    .row:hover .act {
      opacity: 1;
      transform: translateY(-50%);
    }
    .row:hover:has(.act) .time {
      opacity: 0;
    }
  }
  .row[data-rewindable]:active .act {
    transform: translateY(-50%) scale(0.96);
  }

  .ackc[data-touch] .row[data-rewindable] .time,
  .ackc[data-touch] .act[data-quiet] {
    display: none;
  }
  .ackc[data-touch] .act {
    position: static;
    flex: none;
    margin-left: auto;
    opacity: 1;
    transform: none;
  }
  .ackc[data-touch] .row[data-rewindable]:active .act {
    transform: scale(0.96);
  }
  @container ackc (max-width: 420px) {
    .word {
      display: none;
    }
    .act {
      padding: 0 5px;
    }
  }

  .note {
    margin: 0;
    padding: 0 var(--ackc-edge) 8px calc(var(--ackc-gutter) + 26px);
    font-family: var(--font-sans);
    font-size: 12px;
    line-height: 18px;
    color: var(--theme-muted);
    overflow-wrap: anywhere;
  }
  .note[data-tone='failed'] {
    color: var(--theme-danger);
  }

  @keyframes ackc-draw {
    from {
      stroke-dashoffset: 1;
    }
  }
  @keyframes ackc-pop {
    from {
      opacity: 0;
      transform: scale(0.4);
    }
  }
  @keyframes ackc-rise {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
  }
  @keyframes ackc-glint {
    from {
      transform: translateX(-10px) rotate(20deg);
    }
    to {
      transform: translateX(10px) rotate(20deg);
    }
  }
  @keyframes ackc-shine {
    from {
      background-position: 100% 0;
    }
    to {
      background-position: 0 0;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .ackc :global(*),
    .ackc :global(*::before),
    .ackc :global(*::after) {
      animation: none !important;
      transition: none !important;
    }
    .row[data-working] .name {
      background: none;
      color: var(--theme-text);
    }
    .glint {
      display: none;
    }
  }
</style>
