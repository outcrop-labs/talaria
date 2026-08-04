import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from 'react'
import {
  Bot,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  GripVertical,
  MessageSquareText,
  Paperclip,
  RotateCcw,
  X,
} from 'lucide-react'
import { useQuery } from '@tanstack/react-query'
import { MessageAttachments, PendingAttachments } from '@/components/chat/attachments'
import { ChatComposer, type ChatComposerHandle } from '@/components/chat/chat-composer'
import { ScoutComposerControls, type ScoutMode } from '@/components/inbox/scout-composer-controls'
import { Button, buttonClasses } from '@/components/ui/button'
import { Markdown } from '@/components/ui/markdown'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { splitAttachments, uploadFile, type Attachment } from '@/lib/attachments'
import { mergeInboxTimelinePages } from '@/lib/inbox-focus-timeline'
import {
  DEFAULT_INBOX_PANEL_WIDTH,
  MAX_INBOX_PANEL_WIDTH,
  MIN_INBOX_PANEL_WIDTH,
  clampInboxPanelWidth,
  shouldCollapseInboxPanel,
} from '@/lib/inbox-panel-size'
import {
  useInboxFocusConversation,
  type FocusAssistant,
  type FocusItem,
  type InboxTimelineEntry,
} from '@/lib/inbox-focus'
import { useModels } from '@/lib/muse'
import { useSkillLibrary } from '@/lib/workflows'

const PANEL_COLLAPSED_KEY = 'talaria:inbox-chat-collapsed'
const PANEL_COLLAPSED_EVENT = 'talaria:inbox-chat-collapsed'
// v2 adopts the 700px Paper composer as the default while retaining resizing.
const PANEL_WIDTH_KEY = 'talaria:inbox-chat-width-v2'
let collapsedFallback = false
let widthFallback = DEFAULT_INBOX_PANEL_WIDTH

function readPanelCollapsed(): boolean {
  try {
    return window.localStorage.getItem(PANEL_COLLAPSED_KEY) === '1'
  } catch {
    return collapsedFallback
  }
}

function subscribePanelCollapsed(onChange: () => void): () => void {
  window.addEventListener(PANEL_COLLAPSED_EVENT, onChange)
  window.addEventListener('storage', onChange)
  return () => {
    window.removeEventListener(PANEL_COLLAPSED_EVENT, onChange)
    window.removeEventListener('storage', onChange)
  }
}

function usePanelCollapsed() {
  const collapsed = useSyncExternalStore(subscribePanelCollapsed, readPanelCollapsed, () => false)
  const setCollapsed = useCallback((next: boolean) => {
    collapsedFallback = next
    try {
      window.localStorage.setItem(PANEL_COLLAPSED_KEY, next ? '1' : '0')
    } catch {
      /* private mode: keep the in-memory preference for this tab */
    }
    window.dispatchEvent(new Event(PANEL_COLLAPSED_EVENT))
  }, [])
  return { collapsed, setCollapsed }
}

function readPanelWidth(): number {
  try {
    const stored = window.localStorage.getItem(PANEL_WIDTH_KEY)
    return stored === null ? widthFallback : clampInboxPanelWidth(Number(stored))
  } catch {
    return widthFallback
  }
}

function usePanelWidth() {
  const [width, setLocalWidth] = useState(DEFAULT_INBOX_PANEL_WIDTH)
  const committedWidthRef = useRef(DEFAULT_INBOX_PANEL_WIDTH)

  useEffect(() => {
    const syncWidth = () => {
      const next = readPanelWidth()
      committedWidthRef.current = next
      setLocalWidth((current) => current === next ? current : next)
    }
    const syncStoredWidth = (event: StorageEvent) => {
      if (event.key === PANEL_WIDTH_KEY) syncWidth()
    }
    syncWidth()
    window.addEventListener('storage', syncStoredWidth)
    return () => window.removeEventListener('storage', syncStoredWidth)
  }, [])

  const setWidth = useCallback((next: number) => {
    const clamped = clampInboxPanelWidth(next)
    if (clamped === committedWidthRef.current) return
    committedWidthRef.current = clamped
    widthFallback = clamped
    setLocalWidth(clamped)
    try {
      window.localStorage.setItem(PANEL_WIDTH_KEY, String(clamped))
    } catch {
      /* private mode: keep the in-memory preference for this tab */
    }
  }, [])
  return { width, setWidth }
}

export interface InboxChatPanelHandle {
  focus: () => void
  expand: () => void
  insertText: (text: string) => void
}

interface StreamingTurn {
  user: string
  status: string
  content: string
}

export interface InboxCommandOptions {
  focusKey: string | null
  delegateModel: string | null
  responseModel: string | null
  mode: ScoutMode
  attachmentIds: string[]
  refs: Array<{ type: 'kb-doc' | 'artifact'; id: string }>
}

interface AgentMcpSummary {
  id: string
  slug: string
  displayName: string
  servers: Array<{ name: string; extras: string[] }>
}

export const InboxChatPanel = forwardRef<
  InboxChatPanelHandle,
  {
    active: FocusItem | null
    focusMode: boolean
    assistant: FocusAssistant | undefined
    busy: boolean
    notice: string | null
    streaming: StreamingTurn | null
    onSubmit: (instruction: string, options: InboxCommandOptions) => void
    onConfirm: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
    onCancel: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
    onRetry: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
    onUndo: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
  }
>(function InboxChatPanel(
  { active, focusMode, assistant, busy, notice, streaming, onSubmit, onConfirm, onCancel, onRetry, onUndo },
  ref,
) {
  const { collapsed, setCollapsed } = usePanelCollapsed()
  const { width: savedWidth, setWidth: setSavedWidth } = usePanelWidth()
  const conversation = useInboxFocusConversation()
  const { data: agentData, isLoading: agentsLoading } = useAgents()
  const models = useModels()
  const skills = useSkillLibrary()
  const mcp = useQuery({
    queryKey: ['inbox-scout-mcp'],
    queryFn: async (): Promise<{ agents: AgentMcpSummary[] }> => {
      const response = await fetch('/api/mcp', { credentials: 'same-origin' })
      if (!response.ok) return { agents: [] }
      return response.json() as Promise<{ agents: AgentMcpSummary[] }>
    },
    staleTime: 30_000,
  })
  const [delegateModel, setDelegateModel] = useState('')
  const [responseModel, setResponseModel] = useState('')
  const [mode, setMode] = useState<ScoutMode>('normal')
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [attachmentError, setAttachmentError] = useState<string | null>(null)
  const [detachedKey, setDetachedKey] = useState<string | null>(null)
  const [hiddenOutput, setHiddenOutput] = useState(false)
  const [dragWidth, setDragWidth] = useState<number | null>(null)
  const composerRef = useRef<ChatComposerHandle>(null)
  const expandButtonRef = useRef<HTMLButtonElement>(null)
  const scrollerRef = useRef<HTMLDivElement>(null)
  const pinnedRef = useRef(true)
  const lastOutputRef = useRef<string | null>(null)
  const resizeRef = useRef<{
    pointerId: number
    startX: number
    startWidth: number
    latestRawWidth: number
  } | null>(null)
  const resizeFrameRef = useRef<number | null>(null)
  const pendingDragWidthRef = useRef<number | null>(null)
  const attached = Boolean(active && detachedKey !== active.key)
  const toggleActiveDecisionAttachment = useCallback(() => {
    if (!active) return
    setDetachedKey((current) => current === active.key ? null : active.key)
  }, [active])
  const panelWidth = dragWidth ?? savedWidth
  const resizing = dragWidth !== null

  const entries = useMemo(
    () => mergeInboxTimelinePages((conversation.data?.pages ?? []).map((page) => page.entries)),
    [conversation.data?.pages],
  )

  const agents = useMemo(
    () => [
      {
        id: '',
        label: assistant?.name ?? 'Scout',
        role: assistant?.configured ? 'Personal orchestrator' : 'Not configured',
      },
      ...(agentData?.agents ?? []).filter((agent) => agent.id !== assistant?.model),
    ],
    [agentData?.agents, assistant],
  )

  useEffect(() => {
    const available = models.data?.models ?? []
    if (available.length === 0) {
      if (!models.isLoading) setResponseModel('')
      return
    }
    if (available.some((model) => model.id === responseModel)) return
    const preferred = models.data?.effective
    setResponseModel(available.some((model) => model.id === preferred) ? preferred! : available[0]!.id)
  }, [models.data?.effective, models.data?.models, models.isLoading, responseModel])

  const effectiveAgentModel = delegateModel || assistant?.model || ''
  const skillOwner = skills.data?.find((owner) => owner.model === effectiveAgentModel)
  const skillItems = useMemo(() => {
    const owners = skills.data ?? []
    const selected = owners.filter((owner) => owner.owner === 'shared' || owner.model === effectiveAgentModel)
    const seen = new Set<string>()
    return selected.flatMap((owner) => owner.skills.map((skill) => ({
      id: `${owner.owner}:${skill.name}`,
      label: skill.name,
      detail: owner.owner === 'shared' ? 'Shared' : owner.label,
    }))).filter((skill) => {
      if (seen.has(skill.label)) return false
      seen.add(skill.label)
      return true
    })
  }, [effectiveAgentModel, skills.data])
  const mcpItems = useMemo(() => {
    const roster = mcp.data?.agents ?? []
    const selected = roster.find((agent) => agent.slug === skillOwner?.owner)
      ?? roster.find((agent) => agent.displayName === (delegateModel
        ? agentData?.agents.find((agent) => agent.id === delegateModel)?.label
        : assistant?.name))
    return (selected?.servers ?? []).map((server) => ({
      id: `${selected?.id ?? 'agent'}:${server.name}`,
      label: server.name,
      detail: server.extras.includes('built-in') ? 'Built in' : server.extras.includes('managed') ? 'Managed' : undefined,
    }))
  }, [agentData?.agents, assistant?.name, delegateModel, mcp.data?.agents, skillOwner?.owner])

  const addAttachment = useCallback((attachment: Attachment) => {
    setAttachments((current) => current.some((item) => item.id === attachment.id) ? current : [...current, attachment])
    setAttachmentError(null)
  }, [])

  const uploadFiles = useCallback(async (files: File[]) => {
    setAttachmentError(null)
    for (const file of files) {
      const result = await uploadFile(file)
      if ('error' in result) setAttachmentError(result.error)
      else addAttachment(result)
    }
  }, [addAttachment])

  const expand = useCallback(() => {
    setCollapsed(false)
    setHiddenOutput(false)
    window.setTimeout(() => composerRef.current?.focus(), 0)
  }, [setCollapsed])
  const collapse = useCallback(() => {
    setCollapsed(true)
    window.setTimeout(() => expandButtonRef.current?.focus(), 0)
  }, [setCollapsed])

  const beginResize = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    event.preventDefault()
    resizeRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX,
      startWidth: panelWidth,
      latestRawWidth: panelWidth,
    }
    setDragWidth(panelWidth)
  }

  const finishResize = useCallback((pointerId: number, cancelled = false) => {
    const drag = resizeRef.current
    if (!drag || drag.pointerId !== pointerId) return
    resizeRef.current = null
    if (resizeFrameRef.current !== null) window.cancelAnimationFrame(resizeFrameRef.current)
    resizeFrameRef.current = null
    pendingDragWidthRef.current = null
    setDragWidth(null)
    if (cancelled) return
    if (shouldCollapseInboxPanel(drag.latestRawWidth)) collapse()
    else setSavedWidth(drag.latestRawWidth)
  }, [collapse, setSavedWidth])

  useEffect(() => {
    if (!resizing) return
    const previousCursor = document.body.style.cursor
    const previousUserSelect = document.body.style.userSelect
    const moveResize = (event: PointerEvent) => {
      const drag = resizeRef.current
      if (!drag || drag.pointerId !== event.pointerId) return
      const rawWidth = drag.startWidth + event.clientX - drag.startX
      drag.latestRawWidth = rawWidth
      pendingDragWidthRef.current = clampInboxPanelWidth(rawWidth)
      if (resizeFrameRef.current !== null) return
      resizeFrameRef.current = window.requestAnimationFrame(() => {
        resizeFrameRef.current = null
        const next = pendingDragWidthRef.current
        pendingDragWidthRef.current = null
        if (next !== null) setDragWidth(next)
      })
    }
    const completeResize = (event: PointerEvent) => finishResize(event.pointerId)
    const cancelPointerResize = (event: PointerEvent) => finishResize(event.pointerId, true)
    const cancelResize = () => {
      const pointerId = resizeRef.current?.pointerId
      if (pointerId !== undefined) finishResize(pointerId, true)
    }

    document.body.style.cursor = 'col-resize'
    document.body.style.userSelect = 'none'
    window.addEventListener('pointermove', moveResize, true)
    window.addEventListener('pointerup', completeResize, true)
    window.addEventListener('pointercancel', cancelPointerResize, true)
    window.addEventListener('blur', cancelResize)
    return () => {
      document.body.style.cursor = previousCursor
      document.body.style.userSelect = previousUserSelect
      window.removeEventListener('pointermove', moveResize, true)
      window.removeEventListener('pointerup', completeResize, true)
      window.removeEventListener('pointercancel', cancelPointerResize, true)
      window.removeEventListener('blur', cancelResize)
    }
  }, [finishResize, resizing])

  const resizeWithKeyboard = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? 64 : 24
    if (event.key === 'ArrowLeft') {
      event.preventDefault()
      if (panelWidth <= MIN_INBOX_PANEL_WIDTH) collapse()
      else setSavedWidth(panelWidth - step)
    } else if (event.key === 'ArrowRight') {
      event.preventDefault()
      setSavedWidth(panelWidth + step)
    } else if (event.key === 'Home') {
      event.preventDefault()
      collapse()
    } else if (event.key === 'End') {
      event.preventDefault()
      setSavedWidth(MAX_INBOX_PANEL_WIDTH)
    }
  }

  useImperativeHandle(ref, () => ({
    focus: () => composerRef.current?.focus(),
    expand,
    insertText: (text) => composerRef.current?.insertText(text),
  }))

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === '\\') {
        event.preventDefault()
        if (collapsed) expand()
        else collapse()
      } else if (event.key === 'Escape' && !collapsed && window.innerWidth < 1400) {
        event.preventDefault()
        collapse()
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [collapse, collapsed, expand])

  const latestOutput = [...entries].reverse().find((entry) => entry.kind === 'activity' || (entry.kind === 'message' && entry.role === 'assistant'))?.id ?? null
  useEffect(() => {
    if (lastOutputRef.current && latestOutput && latestOutput !== lastOutputRef.current && collapsed) setHiddenOutput(true)
    lastOutputRef.current = latestOutput
  }, [collapsed, latestOutput])

  useEffect(() => {
    if (!pinnedRef.current || !scrollerRef.current) return
    scrollerRef.current.scrollTo({ top: scrollerRef.current.scrollHeight, behavior: streaming ? 'auto' : 'smooth' })
  }, [entries.length, streaming?.content, streaming?.status])

  const loadOlder = async () => {
    const scroller = scrollerRef.current
    const before = scroller?.scrollHeight ?? 0
    await conversation.fetchNextPage()
    requestAnimationFrame(() => {
      if (scroller) scroller.scrollTop += scroller.scrollHeight - before
    })
  }

  const submit = (markdown: string) => {
    const instruction = markdown.trim() || (attachments.length ? 'Review the attached context.' : '')
    if (!instruction || busy) return
    const split = splitAttachments(attachments)
    onSubmit(instruction, {
      focusKey: attached && active ? active.key : null,
      delegateModel: delegateModel || null,
      responseModel: responseModel || null,
      mode,
      attachmentIds: split.attachmentIds,
      refs: split.refs,
    })
    composerRef.current?.clear()
    setAttachments([])
  }

  if (collapsed) {
    return (
      <aside className="relative z-20 flex h-full w-11 shrink-0 flex-col items-center border-r border-line bg-sidebar py-3" aria-label="Scout conversation collapsed">
        <button
          ref={expandButtonRef}
          type="button"
          onClick={expand}
          aria-label="Expand Scout conversation"
          aria-expanded={false}
          className="relative grid h-8 w-8 place-items-center rounded-md text-muted hover:bg-hover hover:text-fg"
        >
          <ChevronRight size={14} />
          {hiddenOutput && <span className="absolute right-0 top-0 h-1.5 w-1.5 rounded-full bg-accent" aria-label="New Scout output" />}
        </button>
        <div className="mt-5 grid h-8 w-8 place-items-center rounded-md border border-line text-muted" title="Scout">
          <Bot size={14} />
        </div>
        {(busy || conversation.data?.pages[0]?.working) && <span className="mt-2 h-1.5 w-1.5 animate-pulse rounded-full bg-success" aria-label="Scout is working" />}
        <MessageSquareText size={14} className="mt-auto text-ink-dim" />
      </aside>
    )
  }

  return (
    <>
      <button type="button" onClick={collapse} aria-label="Close Scout overlay" className="absolute inset-0 z-30 bg-black/45 min-[1400px]:hidden" />
      <aside
        className={cn(
          'absolute inset-y-0 left-0 z-40 flex shrink-0 flex-col border-r border-line bg-sidebar shadow-[var(--theme-shadow-3)] min-[1400px]:relative min-[1400px]:z-20 min-[1400px]:shadow-none',
          !resizing && 'motion-safe:transition-[width,transform] motion-safe:duration-200',
        )}
        style={{ width: `min(${panelWidth}px, calc(100% - 44px))` }}
        aria-label="Scout conversation"
      >
        <div
          role="separator"
          aria-label="Resize Scout conversation"
          aria-orientation="vertical"
          aria-valuemin={MIN_INBOX_PANEL_WIDTH}
          aria-valuemax={MAX_INBOX_PANEL_WIDTH}
          aria-valuenow={Math.round(panelWidth)}
          tabIndex={0}
          title="Drag to resize. Drag left past the minimum to collapse."
          onDoubleClick={() => setSavedWidth(DEFAULT_INBOX_PANEL_WIDTH)}
          onKeyDown={resizeWithKeyboard}
          onPointerDown={beginResize}
          className="group absolute -right-2 inset-y-0 z-50 flex w-4 touch-none cursor-col-resize items-center justify-center focus-visible:!outline-none"
        >
          <span
            className={cn(
              'grid h-12 w-3 place-items-center rounded-full border border-line-strong bg-raised text-ink-dim opacity-0 shadow-[var(--theme-shadow-2)] transition-[opacity,color,background-color] group-hover:opacity-100 group-focus:opacity-100 group-focus:text-fg',
              resizing && 'bg-accent text-surface opacity-100',
            )}
            aria-hidden
          >
            <GripVertical size={10} strokeWidth={1.75} />
          </span>
        </div>
        <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line px-4">
          <span className="grid h-7 w-7 place-items-center rounded-md border border-line text-muted"><Bot size={14} /></span>
          <div className="min-w-0 flex-1">
            <div className="truncate font-sans text-[13px] font-medium text-fg">Scout</div>
            <div className="font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">{focusMode ? 'Inbox conversation' : 'Assistant conversation'}</div>
          </div>
          <span className={cn('h-1.5 w-1.5 rounded-full', busy || conversation.data?.pages[0]?.working ? 'animate-pulse bg-success' : 'bg-line-strong')} aria-hidden />
          <button type="button" onClick={collapse} aria-label="Collapse Scout conversation" aria-expanded className="grid h-8 w-8 place-items-center rounded-md text-muted hover:bg-hover hover:text-fg">
            <ChevronLeft size={14} />
          </button>
        </header>

        <div
          ref={scrollerRef}
          onScroll={(event) => {
            const node = event.currentTarget
            pinnedRef.current = node.scrollHeight - node.scrollTop - node.clientHeight < 96
          }}
          className="min-h-0 flex-1 overflow-y-auto px-4 pb-6 pt-3"
          aria-live="polite"
        >
          {conversation.hasNextPage && (
            <div className="mb-5 flex justify-center">
              <Button size="sm" variant="ghost" onClick={() => void loadOlder()} disabled={conversation.isFetchingNextPage}>
                {conversation.isFetchingNextPage ? 'Loading' : 'Load earlier history'}
              </Button>
            </div>
          )}
          {conversation.isLoading ? (
            <div className="space-y-4 py-4"><Skeleton className="h-16 w-4/5 rounded-lg" /><Skeleton className="ml-auto h-12 w-3/5 rounded-lg" /><Skeleton className="h-24 w-full rounded-lg" /></div>
          ) : entries.length === 0 && !streaming ? (
            <div className="grid min-h-[320px] place-items-center text-center">
              <div className="max-w-xs">
                <span className="mx-auto grid h-10 w-10 place-items-center rounded-full border border-line text-muted"><Bot size={16} /></span>
                <h2 className="mt-4 font-sans text-base font-medium text-fg">{focusMode ? 'Work through Inbox with Scout' : 'Talk with Scout from anywhere'}</h2>
                <p className="mt-2 font-sans text-xs leading-5 text-muted">{focusMode ? 'The active decision is attached by default. Remove it to have a general, non-executing conversation.' : 'This conversation stays with you as you move through Talaria. General messages do not execute tools or mutations.'}</p>
              </div>
            </div>
          ) : (
            <div className="space-y-5">
              {entries.map((entry) => (
                <TimelineEntry
                  key={entry.id}
                  entry={entry}
                  readOnly={!focusMode}
                  onConfirm={onConfirm}
                  onCancel={onCancel}
                  onRetry={onRetry}
                  onUndo={onUndo}
                />
              ))}
              {streaming && (
                <>
                  <div className="ml-auto max-w-[86%] rounded-xl rounded-br-sm border border-line bg-raised px-3 py-2.5 font-sans text-[13px] leading-5 text-fg">
                    <Markdown>{streaming.user}</Markdown>
                  </div>
                  <div className="border-t border-line pt-4">
                    <div className="mb-2 flex items-center gap-2 font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">
                      <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-success" />{streaming.status}
                    </div>
                    {streaming.content && <div className="font-sans text-[13px] leading-5 text-fg"><Markdown>{streaming.content}</Markdown></div>}
                  </div>
                </>
              )}
            </div>
          )}
        </div>

        <div className="shrink-0 border-t border-line bg-sidebar/95 p-2 backdrop-blur">
          {notice && <div role="status" className="mb-2 rounded-md border border-line bg-panel px-3 py-2 font-sans text-[11px] leading-4 text-muted">{notice}</div>}
          {attachmentError && <div role="alert" className="mb-2 rounded-md border border-danger/45 bg-panel px-3 py-2 font-sans text-[11px] leading-4 text-danger">{attachmentError}</div>}
          {active && (
            <div className="mb-2 flex min-w-0 items-center gap-2 rounded-md border border-line bg-surface px-2.5 py-2">
              <Paperclip size={12} className={attached ? 'text-accent' : 'text-ink-dim'} />
              <button type="button" onClick={toggleActiveDecisionAttachment} className="min-w-0 flex-1 truncate text-left font-sans text-[11px] text-muted">
                {attached ? active.question : 'Decision detached — general conversation'}
              </button>
              <button type="button" onClick={toggleActiveDecisionAttachment} aria-label={attached ? 'Detach active decision' : 'Attach active decision'} className="grid h-6 w-6 place-items-center rounded text-ink-dim hover:bg-hover hover:text-fg">
                {attached ? <X size={12} /> : <Paperclip size={12} />}
              </button>
            </div>
          )}
          <div className="flex flex-col gap-2 rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)]">
            <PendingAttachments items={attachments} onRemove={(id) => setAttachments((current) => current.filter((item) => item.id !== id))} />
            <ChatComposer
              ref={composerRef}
              placeholder={attached ? 'Tell Scout what should happen…' : 'Message Scout…'}
              onSubmit={submit}
              onFiles={(files) => void uploadFiles(files)}
              onEscape={() => window.innerWidth < 1400 && collapse()}
              disabled={busy}
              canSend={attachments.length > 0 || undefined}
              controlRail={
                <ScoutComposerControls
                  agents={agents}
                  agentValue={delegateModel}
                  onAgentChange={setDelegateModel}
                  models={models.data?.models ?? []}
                  modelValue={responseModel}
                  onModelChange={setResponseModel}
                  modelsLoading={models.isLoading}
                  mode={mode}
                  onModeChange={setMode}
                  mcpItems={mcpItems}
                  skillItems={skillItems}
                  onAttach={addAttachment}
                  onTranscript={(text) => composerRef.current?.insertText(text)}
                  disabled={busy}
                />
              }
            />
          </div>
          <div className="mt-1.5 flex items-center justify-between gap-2 px-1 font-mono text-[8px] uppercase tracking-[0.06em] text-ink-dim">
            <span>{assistant?.configured ? `${assistant.name ?? 'Scout'} orchestrates` : 'Assistant not configured'}</span>
            <span>{agentsLoading ? 'Loading' : delegateModel ? 'Specialist ready' : attached ? 'Decision attached' : 'No tools'}</span>
          </div>
        </div>
      </aside>
    </>
  )
})

function TimelineEntry({
  entry,
  readOnly,
  onConfirm,
  onCancel,
  onRetry,
  onUndo,
}: {
  entry: InboxTimelineEntry
  readOnly: boolean
  onConfirm: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
  onCancel: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
  onRetry: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
  onUndo: (entry: Extract<InboxTimelineEntry, { kind: 'activity' }>) => void
}) {
  if (entry.kind === 'context') {
    return (
      <div className="flex items-center gap-3 py-1">
        <span className="h-px flex-1 bg-line" />
        <a href={entry.focus.sourceHref} className="max-w-[72%] truncate font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim hover:text-muted">{entry.focus.question}</a>
        <span className="h-px flex-1 bg-line" />
      </div>
    )
  }
  if (entry.kind === 'message') {
    if (entry.role === 'user') {
      return (
        <div className="ml-auto max-w-[86%] rounded-xl rounded-br-sm border border-line bg-raised px-3 py-2.5 font-sans text-[13px] leading-5 text-fg">
          <Markdown>{entry.content}</Markdown>
          <MessageAttachments items={entry.attachments} />
        </div>
      )
    }
    return (
      <div className="border-t border-line pt-4">
        {entry.delegateModel && <div className="mb-2 font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">Consulted {entry.delegateModel}</div>}
        {entry.status === 'error'
          ? <p className="font-sans text-xs text-danger">Scout did not finish this response.</p>
          : <div className="font-sans text-[13px] leading-5 text-fg"><Markdown>{entry.content}</Markdown></div>}
      </div>
    )
  }

  const label = {
    proposal: 'Proposed action',
    confirmation: 'Confirmation required',
    completion: 'Completed',
    failure: 'Action failed',
    cancellation: 'Cancelled',
    undo: 'Undone',
  }[entry.activity]
  return (
    <section className={cn('rounded-lg border bg-panel p-3', entry.activity === 'failure' ? 'border-danger/45' : entry.activity === 'confirmation' ? 'border-accent/55' : 'border-line')} aria-label={label}>
      <div className="font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">{label}</div>
      <h3 className="mt-1 font-sans text-[13px] font-medium text-fg">{entry.title}</h3>
      {entry.message && <p className="mt-1.5 font-sans text-[11px] leading-4 text-muted">{entry.message}</p>}
      {entry.activity === 'confirmation' && (
        <pre className="mt-3 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md border border-line bg-surface p-2.5 font-mono text-[10px] leading-4 text-muted">{exactPreview(entry.details)}</pre>
      )}
      <div className="mt-3 flex flex-wrap items-center gap-2">
        {!readOnly && entry.activity === 'confirmation' && entry.confirmationToken && (
          <>
            <Button size="sm" onClick={() => onConfirm(entry)}>Confirm exact action</Button>
            <Button size="sm" variant="ghost" onClick={() => onCancel(entry)}>Cancel</Button>
          </>
        )}
        {!readOnly && entry.activity === 'failure' && entry.actionId && <Button size="sm" variant="outline" onClick={() => onRetry(entry)}>Retry</Button>}
        {!readOnly && entry.activity === 'completion' && entry.undoExpiresAt
          ? <Button size="sm" variant="ghost" onClick={() => onUndo(entry)}><RotateCcw size={12} /> Undo</Button>
          : entry.activity === 'completion' && <a href={entry.focus.sourceHref} className={buttonClasses({ variant: 'ghost', size: 'sm' })}><ExternalLink size={12} /> View result</a>}
        {(entry.activity === 'failure' || entry.activity === 'cancellation' || (readOnly && (entry.activity === 'proposal' || entry.activity === 'confirmation'))) && <a href={entry.focus.sourceHref} className={buttonClasses({ variant: 'ghost', size: 'sm' })}>Open source</a>}
      </div>
    </section>
  )
}

function exactPreview(value: unknown): string {
  if (typeof value === 'string') return value
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value ?? '')
  }
}
