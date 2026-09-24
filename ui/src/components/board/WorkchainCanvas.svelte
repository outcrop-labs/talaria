<script lang="ts">
  // One workchain as a NODE CANVAS (TALA-35), now the wiring editor (TALA-34):
  // the ports and wires the canvas renders are grabbable. A drag out of an
  // out-port pulls a preview wire that follows the pointer; legal targets
  // highlight their in-ports; a drop on an in-port (or card) draws the edge,
  // a drop on empty canvas opens the new-ticket composer (create-and-connect,
  // no picker — the acceptance gesture), anywhere else cancels. Wires select
  // on click (hover lifts them first) and Delete removes the selected edge;
  // right-clicking a card offers the same via context menu. Cards drag to
  // reposition (persisting through PATCH nodes), the canvas pans (space-drag,
  // or an empty-canvas drag) and zooms (a modified wheel — ctrl/⌘, or the
  // ctrl a trackpad pinch synthesizes; a bare wheel scrolls the page), and
  // zoom-to-fit resets.
  //
  // The pure halves live in lib: workchain-rules.ts (layout, paths, states),
  // wiring-overlay.ts (geometry + viewport math), wiring-canvas.ts (the drop
  // resolution + candidate sets) — this component is the DOM half.
  import { Check, Maximize2, Pause, Pencil, Play, Trash2, ZoomIn, ZoomOut } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import { createTask } from '@/lib/boards.svelte'
  import { assigneeInfo } from '@/lib/assignees'
  import { cn } from '@/lib/cn'
  import { formatShortDate } from '@/lib/format'
  import { toastError } from '@/lib/toast.svelte'
  import { EFFORT_LABEL } from '@/lib/task-const'
  import { statusColorOf, type BoardStatus } from '@/lib/statuses'
  import { isOverdueTask } from '@/components/board/field-pills'
  import {
    addWorkchainEdge,
    addWorkchainStep,
    removeWorkchainEdge,
    removeWorkchainStep,
    updateWorkchain,
  } from '@/lib/workchain-client'
  import {
    autoLayout,
    chainProgress,
    NODE_H,
    NODE_W,
    wirePath,
    wireState,
    type Workchain,
    type WorkchainEdge,
    type WorkchainStep,
  } from '@/lib/workchain-rules'
  import {
    clientToCanvas,
    distanceToWire,
    fitTransform,
    hitTestNode,
    hitTestPort,
    portRadius,
    wheelZoomGesture,
    zoomAt,
  } from '@/lib/wiring-overlay'
  import { edgeCandidateSteps, edgeDropOutcome, WIRE_HIT_RADIUS } from '@/lib/wiring-canvas'
  import type { AgentModel } from '@/lib/agents'
  import type { BoardMember } from '@/lib/boards.svelte'

  let {
    workchain,
    boardId,
    boardStatuses,
    agents,
    members,
    onOpen,
    onChanged,
    managed = false,
    onRename,
    onDelete,
  }: {
    workchain: Workchain
    /** The chain's board — the create-and-connect composer files the new
     *  ticket here before wiring it in. */
    boardId: string
    boardStatuses: BoardStatus[]
    agents: Array<Pick<AgentModel, 'id' | 'label'>>
    members: BoardMember[]
    onOpen: (taskId: string) => void
    /** A write landed; the owner re-reads (it owns the query). */
    onChanged: () => void
    /** Managed mode: the header grows the chain's rename/delete affordances.
     *  The writes and their confirms stay with the caller — onRename and
     *  onDelete are the hooks it gets. */
    managed?: boolean
    /** The chain's new name — trimmed, non-empty, actually changed. */
    onRename?: (name: string) => void
    /** The chain goes; the caller owns the confirm and the request. */
    onDelete?: () => void
  } = $props()

  const info = (step: WorkchainStep) =>
    step.assignees.map((a) => assigneeInfo(a, agents, members))

  const failure = (what: string) => (e: unknown) =>
    toastError(`${what} failed`, e)

  const togglePaused = () =>
    void updateWorkchain(workchain.id, { paused: !workchain.paused })
      .then(onChanged)
      .catch(failure(workchain.paused ? 'Unpausing the chain' : 'Pausing the chain'))

  // ── Managed mode: the rename affordance ──────────────────────────────────
  // The popover's draft, reset to the live name on every open (the trigger
  // click), so a refetch can never leave stale text sitting in the box.
  let renameDraft = $state('')

  const commitRename = () => {
    const name = renameDraft.trim()
    if (name && name !== workchain.name) onRename?.(name)
  }

  const menu = useContextMenu()

  // ── The graph layout ─────────────────────────────────────────────────────
  const byTask = $derived(new Map(workchain.steps.map((s) => [s.taskId, s])))
  const positions = $derived(autoLayout(workchain.steps, workchain.edges))

  // Drag: a moved card writes its new spot (per axis, coalesced server-side).
  // The drag is local-first: positions live in a local overlay so the card
  // follows the cursor 1:1, and the PATCH fires on release.
  let dragId = $state<string | null>(null)
  let dragPos = $state<{ x: number; y: number } | null>(null)
  let dragStart = $state<{ mx: number; my: number; ox: number; oy: number; pointerId: number } | null>(null)

  const nodePos = (taskId: string): { x: number; y: number } =>
    dragId === taskId && dragPos ? dragPos : (positions.get(taskId) ?? { x: 0, y: 0 })

  const startDrag = (taskId: string, e: PointerEvent) => {
    const p = positions.get(taskId) ?? { x: 0, y: 0 }
    dragId = taskId
    dragPos = { ...p }
    dragStart = { mx: e.clientX, my: e.clientY, ox: p.x, oy: p.y, pointerId: e.pointerId }
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }

  const moveDrag = (e: PointerEvent) => {
    if (!dragId || !dragStart || e.pointerId !== dragStart.pointerId) return
    dragPos = { x: dragStart.ox + (e.clientX - dragStart.mx), y: dragStart.oy + (e.clientY - dragStart.my) }
  }

  const endDrag = () => {
    if (!dragId || !dragPos || !dragStart) return
    const { x, y } = dragPos
    const id = dragId
    const moved =
      Math.abs(x - dragStart.ox) > 1 || Math.abs(y - dragStart.oy) > 1
    dragId = null
    dragPos = null
    dragStart = null
    // A click (no real move) skips the write — the open-ticket handler owns
    // clicks, and persisting an auto-layout spot would freeze it as a
    // user placement. Only a real drag writes.
    if (!moved) return
    void updateWorkchain(workchain.id, { nodes: [{ taskId: id, x: Math.round(x), y: Math.round(y) }] })
      .then(onChanged)
      .catch(failure('Moving the card'))
  }

  const canvasSize = $derived({
    w: Math.max(0, ...[...positions.values()].map((p) => p.x + NODE_W)) + 80,
    h: Math.max(0, ...[...positions.values()].map((p) => p.y + NODE_H)) + 80,
  })

  // ── The viewport: pan + zoom (TALA-34) ───────────────────────────────────
  // The transform is translation + scale; every pointer coordinate crosses
  // clientToCanvas before any hit test. A modified wheel (ctrl/⌘ — trackpad
  // pinch included) zooms at the cursor; space-drag
  // pans (and an empty-canvas drag pans too, the n8n convenience). Pan/zoom
  // is session-local state, never persisted.
  let viewport = $state({ x: 0, y: 0, k: 1 })
  let spaceHeld = $state(false)
  let panning = $state(false)
  let panStart = $state<{ mx: number; my: number; vx: number; vy: number; pointerId: number } | null>(null)
  let surface = $state<HTMLDivElement | null>(null)
  let plane = $state<HTMLDivElement | null>(null)

  const onWheel = (e: WheelEvent) => {
    // Zoom is a MODIFIED wheel: ctrlKey (a trackpad pinch synthesizes it)
    // or metaKey (⌘/Win+wheel). A bare wheel returns un-prevented — the
    // page's scroll container takes it.
    if (!wheelZoomGesture(e)) return
    e.preventDefault()
    if (!surface) return
    const rect = surface.getBoundingClientRect()
    const focus = clientToCanvas({ x: e.clientX, y: e.clientY }, rect, viewport)
    // trackpads speak small deltas per event (ctrl+wheel pinch), mice one
    // notch (±100); both should feel like a notch or a nudge, not a jump.
    const step = Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.002))
    viewport = zoomAt(viewport, step, focus)
  }

  const startPan = (e: PointerEvent) => {
    panning = true
    panStart = { mx: e.clientX, my: e.clientY, vx: viewport.x, vy: viewport.y, pointerId: e.pointerId }
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }

  const movePan = (e: PointerEvent) => {
    if (!panning || !panStart || e.pointerId !== panStart.pointerId) return
    viewport = { ...viewport, x: panStart.vx + (e.clientX - panStart.mx), y: panStart.vy + (e.clientY - panStart.my) }
  }

  const endPan = () => {
    panning = false
    panStart = null
  }

  const zoomBy = (factor: number) => {
    const el = surface
    if (!el) return
    // zoom on the viewport's own center, no pointer in play
    const rect = el.getBoundingClientRect()
    const focus = clientToCanvas(
      { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 },
      { left: rect.left, top: rect.top },
      viewport,
    )
    viewport = zoomAt(viewport, factor, focus)
  }

  const zoomToFit = () => {
    const el = surface
    if (!el) return
    const bounds =
      positions.size === 0
        ? { w: NODE_W + 80, h: NODE_H + 80 }
        : {
            w: Math.max(NODE_W, ...[...positions.values()].map((p) => p.x + NODE_W)),
            h: Math.max(NODE_H, ...[...positions.values()].map((p) => p.y + NODE_H)),
          }
    viewport = fitTransform(bounds, { w: el.clientWidth, h: el.clientHeight })
  }

  // ── Wiring: the pointer-event overlay (the ticket's real cost) ───────────
  // One drag gesture: pointerdown on an out-port captures the pointer on the
  // SURFACE, every move re-tests the point in canvas space, and pointerup
  // resolves the drop through edgeDropOutcome. Native HTML5 DnD cannot do
  // port-to-port — this is the replacement, and the hit testing is exact.
  type WireDrag = {
    fromTaskId: string
    pointerId: number
    /** Canvas-space pointer position, live. */
    at: { x: number; y: number }
    /** What the pointer is over right now — the highlight driver. */
    over: { taskId: string; side: 'in' | 'out' | null } | null
  }
  let wireDrag = $state<WireDrag | null>(null)
  /** The empty-canvas composer: opened by a drop on nothing, holds the new
   *  ticket's title, and remembers where the wire was headed. */
  let composer = $state<{ fromTaskId: string; at: { x: number; y: number }; title: string } | null>(null)
  let composerAt = $state<{ left: number; top: number } | null>(null)

  /** The steps this drag may legally land on — the highlight set. */
  const candidates = $derived(
    wireDrag ? edgeCandidateSteps(workchain.steps, workchain.edges, wireDrag.fromTaskId) : [],
  )
  const candidateIds = $derived(new Set(candidates.map((s) => s.taskId)))

  const startWire = (taskId: string, e: PointerEvent) => {
    e.stopPropagation()
    e.preventDefault()
    const rect = surface?.getBoundingClientRect()
    if (!rect) return
    wireDrag = {
      fromTaskId: taskId,
      pointerId: e.pointerId,
      at: clientToCanvas({ x: e.clientX, y: e.clientY }, rect, viewport),
      over: null,
    }
    surface?.setPointerCapture(e.pointerId)
  }

  const moveWire = (e: PointerEvent) => {
    if (!wireDrag || e.pointerId !== wireDrag.pointerId) return
    const rect = surface?.getBoundingClientRect()
    if (!rect) return
    const at = clientToCanvas({ x: e.clientX, y: e.clientY }, rect, viewport)
    // Ports first (the precise target), then the card body, then nothing.
    const portHits: Array<{ taskId: string; side: 'in' | 'out'; pos: { x: number; y: number } }> =
      candidates.map((s) => ({
        taskId: s.taskId,
        side: 'in',
        pos: positions.get(s.taskId) ?? { x: 0, y: 0 },
      }))
    const port = hitTestPort(at, portHits, portRadius(e.pointerType))
    if (port) {
      wireDrag = { ...wireDrag, at, over: { taskId: port.taskId, side: port.side } }
      return
    }
    const nodeId = hitTestNode(at, positions)
    wireDrag = {
      ...wireDrag,
      at,
      over: nodeId && candidateIds.has(nodeId) ? { taskId: nodeId, side: null } : null,
    }
  }

  const endWire = (e: PointerEvent) => {
    if (!wireDrag || e.pointerId !== wireDrag.pointerId) return
    const drag = wireDrag
    wireDrag = null
    const rect = surface?.getBoundingClientRect()
    if (!rect) return
    const at = clientToCanvas({ x: e.clientX, y: e.clientY }, rect, viewport)
    const portHits = candidates.map((s) => ({
      taskId: s.taskId,
      side: 'in' as const,
      pos: positions.get(s.taskId) ?? { x: 0, y: 0 },
    }))
    const port = hitTestPort(at, portHits, portRadius(e.pointerType))
    // The card body counts as a hit for outcome classification even when it
    // cannot take the wire — cycle/cancel must not fall through to the composer.
    const nodeHit = hitTestNode(at, positions)
    const hit = port ?? (nodeHit ? { taskId: nodeHit, side: null } : null)
    const outcome = edgeDropOutcome(workchain.steps, workchain.edges, drag.fromTaskId, hit)
    if (outcome.kind === 'create') {
      void addWorkchainEdge(workchain.id, drag.fromTaskId, outcome.toTaskId)
        .then(onChanged)
        .catch(failure('Wiring the step'))
    } else if (outcome.kind === 'cycle') {
      // The api refuses it too; the drag ending IS the feedback.
    } else if (!port && !nodeHit) {
      // A drop on EMPTY canvas: the create-and-connect gesture — inline
      // composer, no picker (the acceptance signal decides it). A drop that
      // hit a card/port but cannot take the wire just cancels silently.
      openComposer(drag.fromTaskId, at)
    }
  }

  /** Empty-canvas drop: the create-and-connect gesture. The composer floats
   *  where the pointer let go; Enter files the ticket and wires it in. */
  const openComposer = (fromTaskId: string, at: { x: number; y: number }) => {
    const rect = surface?.getBoundingClientRect()
    if (!rect) return
    composer = { fromTaskId, at, title: '' }
    composerAt = {
      left: Math.min(rect.width - 240, Math.max(8, at.x * viewport.k + viewport.x + 12)),
      top: Math.min(rect.height - 64, Math.max(8, at.y * viewport.k + viewport.y - 20)),
    }
  }

  const submitComposer = () => {
    const c = composer
    if (!c) return
    const title = c.title.trim()
    if (!title) return
    composer = null
    void (async () => {
      try {
        const { task } = await createTask(boardId, { title })
        await addWorkchainStep(workchain.id, task.id)
        await addWorkchainEdge(workchain.id, c.fromTaskId, task.id)
        onChanged()
      } catch (e) {
        failure('Creating the wired ticket')(e)
      }
    })()
  }

  const cancelComposer = () => {
    composer = null
  }

  // ── Wire selection & deletion ────────────────────────────────────────────
  let selectedEdge = $state<{ fromTaskId: string; toTaskId: string } | null>(null)
  let hoveredEdge = $state<{ fromTaskId: string; toTaskId: string } | null>(null)

  const clickWire = (e: PointerEvent) => {
    const rect = surface?.getBoundingClientRect()
    if (!rect) return
    const at = clientToCanvas({ x: e.clientX, y: e.clientY }, rect, viewport)
    let best: { fromTaskId: string; toTaskId: string } | null = null
    // Distance is in canvas units; divide by k so the screen-space
    // tolerance stays constant across zoom levels.
    let bestDist = WIRE_HIT_RADIUS / viewport.k
    for (const edge of workchain.edges) {
      const f = positions.get(edge.fromTaskId)
      const t = positions.get(edge.toTaskId)
      if (!f || !t) continue
      const d = distanceToWire(at, f, t, wirePath)
      if (d <= bestDist) {
        bestDist = d
        best = { fromTaskId: edge.fromTaskId, toTaskId: edge.toTaskId }
      }
    }

    selectedEdge = best
  }

  const deleteSelected = () => {
    const sel = selectedEdge
    if (!sel) return
    selectedEdge = null
    void removeWorkchainEdge(workchain.id, sel.fromTaskId, sel.toTaskId)
      .then(onChanged)
      .catch(failure('Cutting the wire'))
  }

  const wireMenu = (e: MouseEvent, edge: WorkchainEdge) => {
    selectedEdge = { fromTaskId: edge.fromTaskId, toTaskId: edge.toTaskId }
    menu.openMenu(e, [{ label: 'Cut wire', danger: true, onSelect: deleteSelected }])
  }

  const cardMenu = (e: MouseEvent, step: WorkchainStep) => {
    menu.openMenu(e, [
      { label: 'Open', onSelect: () => onOpen(step.taskId) },
      { label: 'Remove from chain', danger: true, onSelect: () => removeStep(step.taskId) },
      ...(edgesFrom(step.taskId).length > 0
        ? [{ label: 'Cut all wires from here', danger: true, onSelect: () => cutAllFrom(step.taskId) }]
        : []),
    ])
  }

  // Unlink the ticket from the chain (the ticket itself survives). A miss
  // is a quiet ok per the client contract.
  const removeStep = (taskId: string) => {
    void removeWorkchainStep(workchain.id, taskId)
      .then(onChanged)
      .catch(failure('Removing the step'))
  }

  const edgesFrom = (taskId: string) => workchain.edges.filter((e) => e.fromTaskId === taskId)

  const cutAllFrom = (taskId: string) => {
    const list = edgesFrom(taskId)
    void Promise.all(list.map((e) => removeWorkchainEdge(workchain.id, e.fromTaskId, e.toTaskId)))
      .then(onChanged)
      .catch(failure('Cutting the wires'))
  }

  const onSurfacePointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return
    // space held, or the gesture began on empty canvas (the plane div covers
    // the surface and receives the event): pan
    if (spaceHeld || e.target === surface || e.target === plane) {
      startPan(e)
      return
    }
    // a click (not a pan) may select a wire
    clickWire(e)
  }

</script>

<div class="flex h-full min-w-0 flex-col">
  <!-- Canvas header: the chain's name, its derived progress, its controls —
       the rail header's shape, so the two renders read as one surface. -->
  <div class="flex shrink-0 flex-wrap items-center gap-2">
    <span class="font-sans text-sm font-medium text-fg">{workchain.name}</span>
    <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{chainProgress(workchain)}</span>
    {#if workchain.paused}
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-warning">paused</span>
    {/if}
    {#if workchain.createdBy}
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{workchain.createdBy}</span>
    {/if}
    <span class="flex items-center gap-1">
      <IconButton
        size="sm"
        title={workchain.paused ? 'Unpause chain' : 'Pause chain'}
        onclick={togglePaused}
      >
        {#if workchain.paused}<Play size={14} />{:else}<Pause size={14} />{/if}
      </IconButton>
      <IconButton size="sm" title="Zoom out" onclick={() => zoomBy(1 / 1.2)}>
        <ZoomOut size={14} />
      </IconButton>
      <IconButton size="sm" title="Zoom in" onclick={() => zoomBy(1.2)}>
        <ZoomIn size={14} />
      </IconButton>
      <IconButton size="sm" title="Zoom to fit" onclick={zoomToFit}>
        <Maximize2 size={14} />
      </IconButton>
      {#if managed && onRename}
        <Popover align="left">
          {#snippet trigger(open)}
            <IconButton
              size="sm"
              active={open}
              title="Rename chain"
              onclick={() => (renameDraft = workchain.name)}
            >
              <Pencil size={14} />
            </IconButton>
          {/snippet}
          {#snippet content(close)}
            <div class="w-56 space-y-1">
              <Input
                autofocus
                bind:value={renameDraft}
                placeholder="New name"
                size="sm"
                onkeydown={(e) => {
                  if (e.key === 'Enter') {
                    commitRename()
                    close()
                  }
                }}
              />
              <div class="flex items-center justify-between px-1">
                <span class="font-mono text-[9px] uppercase tracking-[0.05em] text-muted">enter to rename</span>
                <button
                  type="button"
                  class="font-mono text-[9px] uppercase tracking-[0.05em] text-muted hover:text-fg"
                  onclick={close}
                >
                  esc
                </button>
              </div>
            </div>
          {/snippet}
        </Popover>
      {/if}
      {#if managed && onDelete}
        <IconButton size="sm" title="Delete chain" danger onclick={() => onDelete?.()}>
          <Trash2 size={14} />
        </IconButton>
      {/if}
    </span>
  </div>

  <!-- The canvas: wires UNDER the cards, cards above, ports on every card.
       Pointer events ride the surface; the viewport transform carries pan
       and zoom; the overlay is the wiring editor's hit plane. -->
  <!-- svelte-ignore a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
  <div
    bind:this={surface}
    class="relative mt-2 min-h-0 flex-1 overflow-hidden rounded-lg border border-line-subtle bg-canvas"
    style="touch-action: pan-y;"
    role="application"
    aria-label="Workchain canvas — drag between ports to wire steps"
    tabindex="0"
    onpointerdown={onSurfacePointerDown}
    onpointermove={(e) => {
      if (panning) movePan(e)
      else if (wireDrag) moveWire(e)
    }}
    onpointerup={(e) => {
      if (panning) endPan()
      else if (wireDrag) endWire(e)
    }}
    onpointercancel={() => {
      if (panning) endPan()
      else if (wireDrag) wireDrag = null
    }}
    onwheel={onWheel}
    onkeydown={(e) => {
      // The composer's input owns its keys while open — its events bubble
      // here, and the surface must not eat Space or delete a wire mid-title.
      if (composer) return
      if (e.code === 'Space') { e.preventDefault(); spaceHeld = true }
      if (e.key === 'Delete' || e.key === 'Backspace') deleteSelected()
      if (e.key === 'Escape') { selectedEdge = null }
    }}
    onkeyup={(e) => {
      if (e.code === 'Space') spaceHeld = false
    }}
    oncontextmenu={(e) => {
      // empty canvas: no menu noise
      if (e.target === surface || e.target === plane) e.preventDefault()
    }}
  >
    <div
      bind:this={plane}
      class="absolute left-0 top-0 origin-top-left"
      style="transform: translate({viewport.x}px, {viewport.y}px) scale({viewport.k})"
    >
      <div class="relative" style="width: {canvasSize.w}px; height: {canvasSize.h}px">
        <svg class="pointer-events-none absolute inset-0 h-full w-full">
          {#each workchain.edges as e (e.fromTaskId + '>' + e.toTaskId)}
            {@const f = byTask.get(e.fromTaskId)}
            {@const t = byTask.get(e.toTaskId)}
            {#if f && t}
              {@const st = wireState(f, t)}
              {@const sel =
                selectedEdge?.fromTaskId === e.fromTaskId && selectedEdge?.toTaskId === e.toTaskId}
              {@const hov =
                hoveredEdge?.fromTaskId === e.fromTaskId && hoveredEdge?.toTaskId === e.toTaskId}
              <path
                d={wirePath(nodePos(e.fromTaskId), nodePos(e.toTaskId))}
                fill="none"
                stroke-width={sel ? 3.5 : st === 'idle' ? 1.5 : 2}
                class={cn(
                  st === 'idle' && 'stroke-line-strong',
                  st === 'fired' && 'stroke-accent',
                  st === 'done' && 'stroke-success opacity-60',
                  sel && 'stroke-accent',
                  hov && !sel && 'opacity-80',
                )}
                stroke-dasharray={st === 'fired' ? '6 3' : undefined}
              />
              <!-- the wire's own hit plane: wide, transparent, pointer-on -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <path
                d={wirePath(nodePos(e.fromTaskId), nodePos(e.toTaskId))}
                fill="none"
                stroke="transparent"
                stroke-width={WIRE_HIT_RADIUS * 2}
                class="pointer-events-auto cursor-pointer"
                onpointerenter={() => (hoveredEdge = { fromTaskId: e.fromTaskId, toTaskId: e.toTaskId })}
                onpointerleave={() => (hoveredEdge = null)}
                onclick={(ev) => {
                  ev.stopPropagation()
                  selectedEdge = { fromTaskId: e.fromTaskId, toTaskId: e.toTaskId }
                }}
                oncontextmenu={(ev) => wireMenu(ev, e)}
              />
            {/if}
          {/each}
          {#if wireDrag}
            <!-- wirePath treats `to` as a card left edge and adds NODE_H/2,
                 so pre-subtract to land the tip exactly on the pointer dot. -->
            {@const preview = wirePath(nodePos(wireDrag.fromTaskId), {
              x: wireDrag.at.x,
              y: wireDrag.at.y - NODE_H / 2,
            })}
            <path
              d={preview}
              fill="none"
              stroke-width="2"
              stroke-dasharray="5 4"
              class="stroke-accent opacity-90"
            />
            <circle cx={wireDrag.at.x} cy={wireDrag.at.y} r="4" class="fill-accent" />
          {/if}
        </svg>
        {#each workchain.steps as step (step.taskId)}
          {@const pos = nodePos(step.taskId)}
          {@const isCandidate = wireDrag !== null && candidateIds.has(step.taskId)}
          {@const isOver = wireDrag?.over?.taskId === step.taskId && wireDrag.over.side !== 'out'}
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            role="button"
            tabindex={0}
            onclick={() => onOpen(step.taskId)}
            onkeydown={(e) => {
              if (e.key === 'Enter' && e.target === e.currentTarget) onOpen(step.taskId)
            }}
            onpointerdown={(e) => {
              if (e.button === 0 && !spaceHeld && !(e.target as HTMLElement).dataset.port)
                startDrag(step.taskId, e)
            }}
            onpointermove={moveDrag}
            onpointerup={endDrag}
            oncontextmenu={(e) => cardMenu(e, step)}
            class={cn(
              'absolute cursor-grab rounded-lg border border-line bg-panel p-3 text-left transition-colors active:cursor-grabbing',
              step.state === 'head' && 'ring-1 ring-inset ring-accent',
              step.state === 'ready' && 'ring-1 ring-inset ring-accent',
              (step.state === 'done' || step.state === 'archived') && 'opacity-50',
              isCandidate && !isOver && 'border-accent-border',
              isOver && 'border-accent ring-1 ring-inset ring-accent',
            )}
            style="left: {pos.x}px; top: {pos.y}px; width: {NODE_W}px; min-height: {NODE_H}px"
          >
            <!-- Ports: grabbable. The out-port starts a wire drag; the
                 in-port is its drop target. 12px hit spheres on the inside,
                 a larger touch radius via pointerType on the overlay. -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <span
              data-port="in"
              class={cn(
                'absolute top-1/2 -left-1.5 h-3 w-3 -translate-y-1/2 rounded-full border transition-colors',
                isCandidate
                  ? 'border-accent bg-accent-subtle ring-2 ring-accent-border'
                  : 'border-line-strong bg-panel',
              )}
            ></span>
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <span
              data-port="out"
              data-task={step.taskId}
              onpointerdown={(e) => {
                if (e.button === 0) startWire(step.taskId, e)
              }}
              class="absolute top-1/2 -right-1.5 h-3 w-3 cursor-crosshair rounded-full border border-line-strong bg-panel transition-colors hover:border-accent hover:bg-accent-subtle"
            ></span>
            <div class="flex items-center gap-1.5">
              <StatusDot color={statusColorOf(step.status, boardStatuses)} />
              {#if step.ticketRef}
                <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{step.ticketRef}</span>
              {/if}
              {#if step.state === 'done'}
                <Check size={12} class="ml-auto shrink-0 text-success" />
              {:else if step.state === 'head'}
                <span class="ml-auto font-mono text-[9px] uppercase tracking-[0.05em] text-accent">head</span>
              {:else if step.state === 'ready'}
                <span class="ml-auto font-mono text-[9px] uppercase tracking-[0.05em] text-accent">ready</span>
              {/if}
            </div>
            <div class={cn('mt-1 font-sans text-[13px] font-medium leading-snug text-fg', step.state === 'archived' && 'line-through')}>
              {step.title}
            </div>
            <div class="mt-2 flex items-center gap-2">
              {#if step.effort}
                <span class="rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">
                  {EFFORT_LABEL[step.effort]}
                </span>
              {/if}
              {#if step.dueDate}
                {@const overdue = isOverdueTask({ dueDate: step.dueDate, status: step.status }, boardStatuses)}
                <span class={cn('font-mono text-[10px] tracking-[0.05em]', overdue ? 'font-medium text-danger' : 'text-muted')}>
                  {formatShortDate(step.dueDate)}
                </span>
              {/if}
              {#if step.assignees.length > 0}
                <span class="ml-auto flex -space-x-1.5">
                  {#each info(step).slice(0, 3) as a (a.key)}
                    <Avatar name={a.label} class="h-4.5 w-4.5 ring-2 ring-[color:var(--theme-panel)]" />
                  {/each}
                </span>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    </div>
    <!-- The inline new-ticket composer: the empty-canvas drop's
         create-and-connect. Title only; the chain does the rest. Lives at
         SURFACE level (outside the transform) because composerAt is computed
         in surface coordinates. -->
    {#if composer}
      <div
        class="absolute z-10 w-56 rounded-lg border border-accent-border bg-panel p-2 shadow-[var(--theme-shadow-2)]"
        style="left: {composerAt?.left ?? 8}px; top: {composerAt?.top ?? 8}px"
      >
        <Input
          autofocus
          bind:value={composer.title}
          placeholder="New ticket title"
          size="sm"
          onkeydown={(e) => {
            if (e.key === 'Enter') submitComposer()
            else if (e.key === 'Escape') cancelComposer()
          }}
        />
        <div class="mt-1 flex items-center justify-between">
          <span class="font-mono text-[9px] uppercase tracking-[0.05em] text-muted">enter to wire it in</span>
          <button
            type="button"
            class="font-mono text-[9px] uppercase tracking-[0.05em] text-muted hover:text-fg"
            onclick={cancelComposer}
          >
            esc
          </button>
        </div>
      </div>
    {/if}
  </div>
</div>