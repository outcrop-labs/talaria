<script lang="ts">
  // One workchain as a NODE CANVAS (TALA-35), now the wiring editor (TALA-34):
  // the ports and wires the canvas renders are grabbable. A drag out of an
  // out-port pulls a preview wire that follows the pointer; legal targets
  // highlight their in-ports; a drop on an in-port (or card) draws the edge,
  // a drop on empty canvas opens the new-ticket composer (create-and-connect,
  // no picker — the acceptance gesture), anywhere else cancels. Wires select
  // on click (hover lifts them first) and Delete removes the selected edge;
  // right-clicking a card offers the same via context menu. A double-click on
  // empty canvas opens the same composer with no wire — how a chain with no
  // steps gets its first one, since there is no out-port to drag from yet.
  // Cards drag to reposition (persisting through PATCH nodes), the canvas pans
  // (space-drag, or a drag anywhere that is not a card, port or wire) and
  // zooms (a modified wheel — ctrl/⌘, or the ctrl a trackpad pinch
  // synthesizes; a bare wheel scrolls the page). Opening a chain fits it;
  // zoom-to-fit does it again, and Tidy up re-lays the graph on the grid and
  // writes it. Those three float over the canvas's own bottom-left corner —
  // the only controls here, because they are the only ones that need canvas
  // state. The chain's name, progress and management verbs belong to the lens
  // toolbar (Workchains.svelte), which is also where tickets join the chain.
  //
  // The pure halves live in lib: workchain-rules.ts (layout, paths, states),
  // wiring-overlay.ts (geometry + viewport math), wiring-canvas.ts (the drop
  // resolution + candidate sets) — this component is the DOM half.
  import { Check, Maximize2, Wand, ZoomIn, ZoomOut } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Input from '@/components/ui/Input.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
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
    gridLayout,
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
    canEdit = false,
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
    /** Owner/editor: the canvas writes (wires, placements, Tidy up). A reader
     *  still pans, zooms and opens tickets. */
    canEdit?: boolean
  } = $props()

  /** Show the whole graph. The lens calls this after a ticket joins the chain
   *  — "bring this task in" should end with the reader looking at it, not
   *  hunting for wherever the grid put it. */
  export function revealAll(): void {
    fitSoon()
  }

  /** Instance-scoped prefix for the svg marker ids — two canvases in one
   *  document must not collide on `#wire-arrow-idle`. */
  const uid = $props.id()

  const info = (step: WorkchainStep) =>
    step.assignees.map((a) => assigneeInfo(a, agents, members))

  const failure = (what: string) => (e: unknown) =>
    toastError(`${what} failed`, e)

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
  /** The card a drag just moved. A pointerup ends the drag, but the browser
   *  still fires the CLICK the gesture began — and the card's click opens the
   *  ticket. Without this every card drag ended in the ticket overlay. */
  let draggedId: string | null = null

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
    // Screen pixels ÷ the zoom: the card must sit under the cursor at every
    // scale, and positions are canvas units.
    dragPos = {
      x: dragStart.ox + (e.clientX - dragStart.mx) / viewport.k,
      y: dragStart.oy + (e.clientY - dragStart.my) / viewport.k,
    }
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
    draggedId = id
    void updateWorkchain(workchain.id, { nodes: [{ taskId: id, x: Math.round(x), y: Math.round(y) }] })
      .then(onChanged)
      .catch(failure('Moving the card'))
  }

  /** The card's click, minus the one a drag leaves behind. */
  const clickCard = (taskId: string) => {
    if (draggedId === taskId) {
      draggedId = null
      return
    }
    draggedId = null
    onOpen(taskId)
  }

  /** The graph's own box in CANVAS space — from its real top-left, which is
   *  negative whenever a card was dragged left of the origin (the api stores
   *  those coordinates on purpose). The svg is laid over exactly this box, so
   *  nothing is clipped out of it. */
  const canvasBox = $derived.by(() => {
    const pts = [...positions.values()]
    if (pts.length === 0) return { x: 0, y: 0, w: NODE_W, h: NODE_H }
    const xs = pts.map((p) => p.x)
    const ys = pts.map((p) => p.y)
    const x = Math.min(...xs)
    const y = Math.min(...ys)
    return {
      x,
      y,
      w: Math.max(...xs) + NODE_W - x,
      h: Math.max(...ys) + NODE_H - y,
    }
  })
  /** The drawing plane, with a margin so a wire leaving the outermost card
   *  has room and the fit never puts a card flush against the frame. */
  const PLANE_PAD = 80
  const planeBox = $derived({
    x: canvasBox.x - PLANE_PAD,
    y: canvasBox.y - PLANE_PAD,
    w: canvasBox.w + PLANE_PAD * 2,
    h: canvasBox.h + PLANE_PAD * 2,
  })

  /** Tidy up: re-lay the whole chain on the grid and PERSIST it, so the
   *  arrangement survives the read. Free placement makes a mess eventually —
   *  overlapping cards, a successor left of its predecessor — and this is the
   *  one gesture that undoes all of it. */
  const tidyUp = () => {
    const grid = gridLayout(workchain.steps, workchain.edges)
    const nodes = workchain.steps.map((s) => {
      const p = grid.get(s.taskId) ?? { x: 0, y: 0 }
      return { taskId: s.taskId, x: Math.round(p.x), y: Math.round(p.y) }
    })
    if (nodes.length === 0) return
    void updateWorkchain(workchain.id, { nodes })
      .then(() => {
        onChanged()
        fitSoon()
      })
      .catch(failure('Tidying the canvas'))
  }

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
  /** The canvas's dot pitch, in canvas units — the background rides the
   *  viewport so panning and zooming move the ground with the graph. */
  const GRID = 24

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
    if (!el || el.clientWidth === 0) return
    viewport = fitTransform(canvasBox, { w: el.clientWidth, h: el.clientHeight })
  }

  /** Fit after the DOM has the new graph — a write's re-read lands a frame
   *  later, and fitting the old box would leave the new card off screen. */
  const fitSoon = () => {
    requestAnimationFrame(() => requestAnimationFrame(zoomToFit))
  }

  // The FIRST paint of a chain fits it. Without this a chain opened at
  // translate(0,0) scale(1) put its entry card flush in the top-left corner
  // and anything the user had dragged out of the default column off screen
  // entirely — a canvas that reads as empty. Re-arms per chain (the switcher
  // swaps `workchain.id` under this component), never on a later read: a
  // refetch must not yank the viewport the reader just panned.
  let fittedChain: string | null = null
  $effect(() => {
    const id = workchain.id
    if (!surface || fittedChain === id) return
    fittedChain = id
    fitSoon()
  })

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
  /** The empty-canvas composer: opened by a drop on nothing (or a
   *  double-click on a chain with nothing to drag from — then `fromTaskId` is
   *  null and no wire is drawn), holds the new ticket's title, and remembers
   *  where on the canvas it was opened so the card lands THERE. */
  let composer = $state<{ fromTaskId: string | null; at: { x: number; y: number }; title: string } | null>(null)
  let composerAt = $state<{ left: number; top: number } | null>(null)

  /** The steps this drag may legally land on — the highlight set. */
  const edgeCandidates = $derived(
    wireDrag ? edgeCandidateSteps(workchain.steps, workchain.edges, wireDrag.fromTaskId) : [],
  )
  const candidateIds = $derived(new Set(edgeCandidates.map((s) => s.taskId)))

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
      edgeCandidates.map((s) => ({
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
    const portHits = edgeCandidates.map((s) => ({
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
  const openComposer = (fromTaskId: string | null, at: { x: number; y: number }) => {
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
        // wire: false — this gesture names its own wire (or none). The api's
        // default tail edge would hand the new card a second predecessor it
        // was never dragged from.
        await addWorkchainStep(workchain.id, task.id, { wire: false })
        if (c.fromTaskId) await addWorkchainEdge(workchain.id, c.fromTaskId, task.id)
        // The card lands WHERE IT WAS DROPPED. Without this the new step is
        // unplaced, the grid puts it in whatever column its level says, and
        // the gesture's whole point — "a node, here" — is lost: it appeared
        // somewhere else, often on top of another card.
        await updateWorkchain(workchain.id, {
          nodes: [{ taskId: task.id, x: Math.round(c.at.x - NODE_W / 2), y: Math.round(c.at.y - NODE_H / 2) }],
        })
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

  /** Empty canvas = anything that is not a card, a port or a wire. The
   *  identity test this replaces (`e.target === surface || e.target === plane`)
   *  almost never matched: the plane's sized child covers the whole graph
   *  box, so a press on the blank space BETWEEN cards hit that div and the
   *  canvas refused to pan — the one gesture an n8n reader tries first. */
  const onEmptyCanvas = (target: EventTarget | null): boolean =>
    target instanceof Element && !target.closest('[data-node], [data-port], [data-wire]')

  const onSurfacePointerDown = (e: PointerEvent) => {
    if (e.button !== 0) return
    if (spaceHeld || onEmptyCanvas(e.target)) {
      // An empty-canvas press deselects before it pans: the wire hit test
      // already missed, so nothing here is selected any more.
      selectedEdge = null
      startPan(e)
      return
    }
    // Not empty canvas, so: a wire, a port, or a card. Only a press on a WIRE
    // selects one — clickWire's nearest-wire search then picks the right
    // strand where several run close together. A press on a card is the
    // card's (drag, or open), and selecting the wire passing invisibly
    // underneath it would hand the reader a selection they cannot see.
    if (e.target instanceof Element && e.target.closest('[data-wire]')) clickWire(e)
  }

  /** Double-click on empty canvas seeds a node. The acceptance gesture is
   *  "authored entirely by dragging", and a chain with NO steps has no
   *  out-port to drag from — the composer was unreachable and the picker was
   *  the only way in. This is the same composer, with no wire to draw. */
  const onSurfaceDoubleClick = (e: MouseEvent) => {
    if (!onEmptyCanvas(e.target)) return
    const rect = surface?.getBoundingClientRect()
    if (!rect) return
    openComposer(null, clientToCanvas({ x: e.clientX, y: e.clientY }, rect, viewport))
  }

</script>

<!-- The canvas is the whole component: one surface, full bleed. Its chain's
     name, progress and management verbs live on the lens toolbar above it
     (Workchains.svelte) — a header row here would only repeat them and cost
     the canvas the height. What stays is what needs canvas state, floating
     over its own bottom-left corner: zoom, fit, tidy. -->
<!-- The canvas: wires UNDER the cards, cards above, ports on every card.
     Pointer events ride the surface; the viewport transform carries pan
     and zoom; the overlay is the wiring editor's hit plane. -->
<!-- svelte-ignore a11y_no_static_element_interactions, a11y_no_noninteractive_element_interactions -->
<div
  bind:this={surface}
  class="relative h-full w-full overflow-hidden rounded-lg border border-line-subtle bg-surface"
  style="touch-action: pan-y; background-image: radial-gradient(var(--theme-border) 1px, transparent 1.2px); background-size: {GRID * viewport.k}px {GRID * viewport.k}px; background-position: {viewport.x}px {viewport.y}px;"
  role="application"
  aria-label="Workchain canvas — drag between ports to wire steps"
  tabindex="0"
  ondblclick={onSurfaceDoubleClick}
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
    if (onEmptyCanvas(e.target)) e.preventDefault()
  }}
>
  <div
    class="absolute left-0 top-0 origin-top-left"
    style="transform: translate({viewport.x}px, {viewport.y}px) scale({viewport.k})"
  >
    <!-- The wire plane, laid over the graph's REAL box — including the
         negative quadrant a dragged card reaches. An `inset-0` svg sized by
         a positives-only box clipped every wire that left it. -->
    <svg
      class="pointer-events-none absolute"
      style="left: {planeBox.x}px; top: {planeBox.y}px; width: {planeBox.w}px; height: {planeBox.h}px"
      viewBox="{planeBox.x} {planeBox.y} {planeBox.w} {planeBox.h}"
    >
      <!-- Arrowheads, one per wire state. A chain's whole point is
           direction, and an undecorated bezier between two cards reads
           the same both ways. Ids are instance-scoped: two canvases on a
           page must not share a marker. -->
      <defs>
        {#each [['idle', 'var(--theme-border-strong)'], ['fired', 'var(--theme-accent)'], ['done', 'var(--theme-success)'], ['sel', 'var(--theme-accent)']] as [name, fill] (name)}
          <!-- userSpaceOnUse: the head is a fixed 10 canvas units
               whatever the stroke weight, and refX sets it back far
               enough to clear the in-port it points at (which sits ON
               the path's end point). -->
          <marker
            id="{uid}-{name}"
            viewBox="0 0 10 10"
            refX="19"
            refY="5"
            markerWidth="10"
            markerHeight="10"
            markerUnits="userSpaceOnUse"
            orient="auto-start-reverse"
          >
            <path d="M 0 1 L 10 5 L 0 9 z" fill={fill} />
          </marker>
        {/each}
      </defs>
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
            stroke-width={sel ? 3.5 : hov ? 2.75 : 2}
            marker-end="url(#{uid}-{sel ? 'sel' : st})"
            class={cn(
              // A hairline token on the canvas ground was all but
              // invisible: idle wires now carry the strong hairline at
              // full weight, and hover lifts them before a click.
              st === 'idle' && (hov || sel ? 'stroke-accent' : 'stroke-line-strong'),
              st === 'fired' && 'stroke-accent',
              st === 'done' && 'stroke-success opacity-70',
              sel && 'stroke-accent',
            )}
            stroke-dasharray={st === 'fired' ? '6 3' : undefined}
          />
          <!-- the wire's own hit plane: wide, transparent, pointer-on -->
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <path
            data-wire
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
        data-node={step.taskId}
        role="button"
        tabindex={0}
        onclick={() => clickCard(step.taskId)}
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
        <!-- touch-action: none on the PORT only — a finger landing on this
             12px target means to pull a wire, and the surface's pan-y would
             cancel the drag the moment it moved vertically. Cards and canvas
             keep pan-y, so the page still scrolls under a finger. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span
          data-port="out"
          data-task={step.taskId}
          onpointerdown={(e) => {
            if (e.button === 0) startWire(step.taskId, e)
          }}
          style="touch-action: none;"
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
            <!-- The chain's human/agent mix, at a glance (TALA-35): the
                 same Avatar everywhere else uses, with an accent hairline
                 on the agents. A chain whose middle is gold is a chain a
                 fleet is running. -->
            <span class="ml-auto flex -space-x-1.5" title={info(step).map((a) => (a.human ? a.label : `${a.label} (agent)`)).join(', ')}>
              {#each info(step).slice(0, 3) as a (a.key)}
                <Avatar
                  name={a.label}
                  class={cn(
                    'h-4.5 w-4.5 ring-2 ring-[color:var(--theme-panel)]',
                    !a.human && 'border-accent-border text-accent',
                  )}
                />
              {/each}
            </span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
  <!-- The inline new-ticket composer: the empty-canvas drop's
       create-and-connect. Title only; the chain does the rest. Lives at
       SURFACE level (outside the transform) because composerAt is computed
       in surface coordinates. -->
  {#if workchain.steps.length === 0 && !composer}
    <!-- The empty chain's hint: the seed paths a blank canvas has — a
         double-click anywhere (the composer, no picker), or the toolbar's
         Add tickets menu for a ticket already on the board. -->
    <div class="pointer-events-none absolute inset-0 flex items-center justify-center">
      <p class="font-mono text-[10px] uppercase tracking-[0.08em] text-muted">
        No steps yet — double-click the canvas, or add a ticket from the toolbar
      </p>
    </div>
  {/if}
  <!-- The viewport controls, floating over the canvas's own corner: they need
       canvas state (the transform, the graph's box), so they cannot move up to
       the lens toolbar with the chain's verbs — and a header row for four
       icon buttons would have cost the surface its full bleed. -->
  <div
    class="absolute bottom-3 left-3 z-10 flex items-center gap-0.5 rounded-lg border border-line bg-panel p-1 shadow-[var(--theme-shadow-1)]"
  >
    <IconButton size="sm" title="Zoom out" onclick={() => zoomBy(1 / 1.2)}>
      <ZoomOut size={14} />
    </IconButton>
    <IconButton size="sm" title="Zoom in" onclick={() => zoomBy(1.2)}>
      <ZoomIn size={14} />
    </IconButton>
    <IconButton size="sm" title="Zoom to fit" onclick={zoomToFit}>
      <Maximize2 size={14} />
    </IconButton>
    {#if canEdit && workchain.steps.length > 0}
      <IconButton size="sm" title="Tidy up — lay the chain back on the grid" onclick={tidyUp}>
        <Wand size={14} />
      </IconButton>
    {/if}
  </div>
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
  <!-- This surface owns the card/wire menus: its controller's open state
       renders here (the portal lifts the panel to <body>). Without this,
       cardMenu/wireMenu set state nothing displays — the TALA-34 menus
       never had a renderer. -->
  <ContextMenu {menu} />
</div>