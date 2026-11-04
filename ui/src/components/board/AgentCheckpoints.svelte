<script lang="ts">
  import { untrack, type Component } from 'svelte'
  import {
    FileText,
    FlaskConical,
    GitBranch,
    GitPullRequest,
    Globe,
    MessagesSquare,
    Pencil,
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
    ROW,
    summarize,
    x,
    y,
    type Checkpoint,
    type Placed,
    type ToolKind,
  } from '@/lib/agent-checkpoints'
  import { prefersReducedMotion } from '@/lib/motion'

  let {
    checkpoints,
    live = false,
  }: {
    checkpoints: Checkpoint[]
    live?: boolean
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

  const placed = $derived(layout(checkpoints))
  const sum = $derived(summarize(placed))
  const headline = $derived(
    [plural(sum.checkpoints, 'checkpoint'), sum.attempts > 1 && plural(sum.attempts, 'attempt')]
      .filter(Boolean)
      .join(' · '),
  )
  const pitch = $derived(
    sum.lanes > 1 && width ? Math.min(LANE, (width * MAX_GUTTER_SHARE - GUTTER_INSET) / (sum.lanes - 1)) : LANE,
  )
  const head = $derived(placed.at(-1))
  const onPath = $derived(livePath(placed))
  const scrolls = $derived(placed.length > MAX_ROWS)

  const rowsAtOpen = untrack(() => checkpoints.length)

  const hasLine = (p: Placed): p is Placed & { from: number; fromLane: number } =>
    p.from !== undefined && p.fromLane !== undefined

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
    const rows = placed.length
    if (!list || !scrolls || !following) return
    const smooth = landed && !prefersReducedMotion()
    landed = true
    list.scrollTo({ top: rows * ROW + PAD, behavior: smooth ? 'smooth' : 'auto' })
  })
</script>

<section
  class="ackc rounded-lg border border-line-subtle"
  aria-labelledby="{uid}-title"
  bind:clientWidth={width}
  style:--ackc-row="{ROW}px"
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
      style:--ackc-rows={placed.length}
      data-scrolls={scrolls ? '' : undefined}
      onscroll={onScroll}
    >
      <svg class="rail" width={x(sum.lanes, pitch)} height={placed.length * ROW} aria-hidden="true">
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
              d={path(p, pitch)}
              pathLength={1}
              data-path={onPath.has(p.row) ? '' : undefined}
              data-fork={p.lane !== p.fromLane ? '' : undefined}
              data-new={p.row >= rowsAtOpen ? '' : undefined}
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
            data-new={p.row >= rowsAtOpen ? '' : undefined}
          >
            <circle cx={x(p.lane, pitch)} cy={y(p.row)} r={isHead ? 4.5 : 3.5} />
            {#if isHead}
              <clipPath id="{uid}-clip-{p.row}">
                <circle cx={x(p.lane, pitch)} cy={y(p.row)} r={4.5} />
              </clipPath>
              <g clip-path="url(#{uid}-clip-{p.row})">
                <rect
                  class="glint"
                  x={x(p.lane, pitch) - 4}
                  y={y(p.row) - 8}
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
          <li
            class="row"
            data-path={onPath.has(p.row) ? '' : undefined}
            data-status={p.cp.status}
            data-working={p.cp.status === 'running' ? '' : undefined}
            data-new={p.row >= rowsAtOpen ? '' : undefined}
            aria-current={isHead ? 'step' : undefined}
          >
            <span class="tool" data-kind={p.cp.tool}>
              <svg width="11" height="11" aria-hidden="true"><use href="#{uid}-{p.cp.tool}" width="11" height="11" /></svg>
            </span>
            <span class="name" title={p.cp.title}>{p.cp.title}</span>
            {#if p.attempt}<Chip>Attempt {p.attempt}</Chip>{/if}
            {#if p.cp.status === 'failed'}<Chip tone="danger">Failed</Chip>{/if}
            {#if p.cp.status === 'stopped'}<Chip>Stopped</Chip>{/if}
            {#if p.cp.status === 'running'}<span class="sr-only">Running</span>{/if}
            {#if !onPath.has(p.row)}<span class="sr-only">Left behind</span>{/if}
            <span class="time">{clock(p.cp.at)}</span>
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
    height: calc(var(--ackc-rows) * var(--ackc-row) + var(--ackc-pad));
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
    transition: stroke 200ms ease-out;
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
  .dot circle {
    fill: var(--theme-panel);
    stroke: var(--ackc-rail);
    stroke-width: 1.5;
    transition:
      fill 200ms ease-out,
      stroke 200ms ease-out;
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

  .row {
    content-visibility: auto;
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--ackc-row);
    padding: 0 var(--ackc-edge) 0 var(--ackc-gutter);
    transition: padding-left 240ms var(--ackc-ease);
  }
  .row[data-new] {
    animation: ackc-rise 180ms var(--ackc-ease) both;
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
  .row:hover::before {
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
  }
  .name {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-sans);
    color: var(--theme-text);
    white-space: nowrap;
    text-overflow: ellipsis;
    transition: color 150ms ease-out;
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
  .row:not([data-path]):hover .name,
  .row:hover .time {
    color: var(--theme-text);
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
    transition: color 150ms ease-out;
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
