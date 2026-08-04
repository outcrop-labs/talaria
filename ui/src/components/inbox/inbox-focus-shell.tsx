import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { InboxChatPanel, type InboxChatPanelHandle } from '@/components/inbox/inbox-chat-panel'
import {
  runInboxFocusAction,
  streamInboxFocusCommand,
  updateInboxFocusState,
  useInboxFocus,
  type FocusAction,
  type FocusActionResult,
  type FocusItem,
  type FocusQueue,
  type InboxTimelineEntry,
} from '@/lib/inbox-focus'

export const INBOX_SNOOZE_OPTIONS = [
  { label: '1 hour', value: 60 * 60_000 },
  { label: 'Tomorrow', value: 24 * 60 * 60_000 },
  { label: 'Next week', value: 7 * 24 * 60 * 60_000 },
] as const

function sourceTypeFromFocusKey(focusKey: string | null | undefined): FocusItem['sourceType'] | undefined {
  return focusKey?.split(':', 1)[0] as FocusItem['sourceType'] | undefined
}

interface InboxFocusWorkspaceValue {
  data: FocusQueue | undefined
  isLoading: boolean
  isError: boolean
  refetch: () => Promise<unknown>
  orderedItems: FocusItem[]
  active: FocusItem | null
  recommendedAction: FocusAction | null
  busyAction: string | null
  snoozeMs: number
  setSnoozeMs: (value: number) => void
  performAction: (item: FocusItem, actionId: string, payload?: unknown) => Promise<void>
  snooze: () => Promise<void>
  skip: () => void
}

const InboxFocusWorkspaceContext = createContext<InboxFocusWorkspaceValue | null>(null)

export function useInboxFocusWorkspace(): InboxFocusWorkspaceValue {
  const value = useContext(InboxFocusWorkspaceContext)
  if (!value) throw new Error('useInboxFocusWorkspace must be used inside InboxFocusShell')
  return value
}

export function InboxFocusShell({ children, attachActiveDecision }: { children: ReactNode; attachActiveDecision: boolean }) {
  const queryClient = useQueryClient()
  const { data, isLoading, isError, refetch } = useInboxFocus()
  const [skippedKeys, setSkippedKeys] = useState<string[]>([])
  const [snoozeMs, setSnoozeMs] = useState<number>(INBOX_SNOOZE_OPTIONS[0].value)
  const [busyAction, setBusyAction] = useState<string | null>(null)
  const [failure, setFailure] = useState<string | null>(null)
  const [assistantMessage, setAssistantMessage] = useState<string | null>(null)
  const [streaming, setStreaming] = useState<{ user: string; status: string; content: string } | null>(null)
  const panelRef = useRef<InboxChatPanelHandle>(null)
  const busyRef = useRef<string | null>(null)
  const commandAbortRef = useRef<AbortController | null>(null)

  useEffect(() => () => commandAbortRef.current?.abort(), [])

  const beginBusy = useCallback((action: string, replace = false): boolean => {
    if (busyRef.current && !replace) return false
    busyRef.current = action
    setBusyAction(action)
    return true
  }, [])

  const endBusy = useCallback((action: string) => {
    if (busyRef.current !== action) return
    busyRef.current = null
    setBusyAction(null)
  }, [])

  const items = useMemo(() => data?.items ?? [], [data?.items])
  useEffect(() => {
    const keySet = new Set(items.map((item) => item.key))
    setSkippedKeys((previous) => {
      const next = previous.filter((key) => keySet.has(key))
      return next.length === previous.length && next.every((key, index) => key === previous[index]) ? previous : next
    })
  }, [items])

  const orderedItems = useMemo(() => {
    const skipped = new Set(skippedKeys)
    const byKey = new Map(items.map((item) => [item.key, item]))
    return [...items.filter((item) => !skipped.has(item.key)), ...skippedKeys.flatMap((key) => byKey.get(key) ?? [])]
  }, [items, skippedKeys])
  const active = orderedItems[0] ?? null
  const recommendedAction = active?.actions.find((action) => action.id === active.recommendedActionId) ?? active?.actions[0] ?? null

  useEffect(() => {
    if (!active || !attachActiveDecision) return
    void updateInboxFocusState({ sourceType: active.sourceType, sourceId: active.sourceId, viewed: true }).catch(() => {})
  }, [active?.key, attachActiveDecision])

  const refreshQueue = useCallback(async (sourceType?: FocusItem['sourceType']) => {
    const invalidations = [
      refetch(),
      queryClient.invalidateQueries({ queryKey: ['inbox-focus-summary'] }),
    ]
    if (sourceType === 'task') invalidations.push(queryClient.invalidateQueries({ queryKey: ['home'] }))
    if (sourceType === 'notification') invalidations.push(queryClient.invalidateQueries({ queryKey: ['notifications'] }))
    if (sourceType === 'channel') invalidations.push(queryClient.invalidateQueries({ queryKey: ['channels'] }))
    if (sourceType === 'approval') invalidations.push(queryClient.invalidateQueries({ queryKey: ['google-pending'] }))
    await Promise.all(invalidations)
  }, [queryClient, refetch])

  const refreshConversation = useCallback(
    () => queryClient.invalidateQueries({ queryKey: ['inbox-focus-conversation'] }),
    [queryClient],
  )

  const receiveResult = useCallback(
    async (sourceType: FocusItem['sourceType'] | undefined, result: FocusActionResult) => {
      if (result.status === 'failed') {
        setFailure(result.message ?? 'That action did not complete.')
        panelRef.current?.expand()
        return
      }
      if (result.status === 'stale') {
        setFailure(result.message ?? 'That item changed. The queue has been refreshed.')
        await refreshQueue(sourceType)
        return
      }
      if (result.status === 'completed') {
        setFailure(null)
        setAssistantMessage(null)
        await refreshQueue(sourceType)
      }
    },
    [refreshQueue],
  )

  const performAction = useCallback(
    async (
      item: FocusItem,
      actionId: string,
      payload?: unknown,
      options: { commandDecisionId?: string; replaceBusy?: boolean } = {},
    ) => {
      if (actionId === 'reply' && payload === undefined) {
        setAssistantMessage('Write the reply below. Scout will show the exact message for confirmation before posting it.')
        panelRef.current?.expand()
        panelRef.current?.insertText('Reply: ')
        return
      }
      if (!beginBusy(actionId, options.replaceBusy)) return
      setFailure(null)
      try {
        const result = await runInboxFocusAction({
          key: item.key,
          actionId,
          payload,
          commandDecisionId: options.commandDecisionId,
        })
        if (result.status === 'confirmation_required') panelRef.current?.expand()
        else await receiveResult(item.sourceType, result)
        await refreshConversation()
      } catch (error) {
        setFailure(error instanceof Error ? error.message : 'The action failed.')
      } finally {
        endBusy(actionId)
      }
    },
    [beginBusy, endBusy, receiveResult, refreshConversation],
  )

  const snooze = useCallback(async () => {
    if (!active || !beginBusy('snooze')) return
    try {
      const result = await updateInboxFocusState({
        sourceType: active.sourceType,
        sourceId: active.sourceId,
        snoozedUntil: new Date(Date.now() + snoozeMs).toISOString(),
      })
      setAssistantMessage(`Snoozed for ${INBOX_SNOOZE_OPTIONS.find((option) => option.value === snoozeMs)?.label.toLowerCase()}.`)
      if (result.timelineEntry) await refreshConversation()
      await refreshQueue()
    } catch (error) {
      setFailure(error instanceof Error ? error.message : 'The item could not be snoozed.')
    } finally {
      endBusy('snooze')
    }
  }, [active, beginBusy, endBusy, refreshConversation, refreshQueue, snoozeMs])

  const skip = useCallback(() => {
    if (!active || orderedItems.length < 2 || busyRef.current) return
    setSkippedKeys((previous) => [...previous.filter((key) => key !== active.key), active.key])
    setFailure(null)
    setAssistantMessage(null)
  }, [active, orderedItems.length])

  const submitCommand = async (
    instruction: string,
    options: { focusKey: string | null; delegateModel: string | null; delegateTier: string | null },
  ) => {
    const trimmedInstruction = instruction.trim()
    if (!trimmedInstruction || !beginBusy('command')) return
    setFailure(null)
    setAssistantMessage(null)
    setStreaming({ user: instruction, status: options.focusKey ? 'Reviewing the active decision' : 'Thinking with Scout', content: '' })
    const controller = new AbortController()
    commandAbortRef.current = controller
    let commandResult: Exclude<import('@/lib/inbox-focus').FocusCommandResponse, { status: 'stale' }> | null = null
    let staleMessage: string | null = null
    try {
      for await (const event of streamInboxFocusCommand({
        key: options.focusKey,
        instruction: trimmedInstruction,
        delegateModel: options.delegateModel,
        delegateTier: options.delegateTier,
      }, controller.signal)) {
        if (event.type === 'status') setStreaming((current) => current ? { ...current, status: event.label } : current)
        if (event.type === 'content') setStreaming((current) => current ? { ...current, content: current.content + event.text } : current)
        if (event.type === 'error') throw new Error(event.message)
        if (event.type === 'done' && event.result) {
          if ('status' in event.result) staleMessage = event.result.message
          else commandResult = event.result
        }
      }
      await refreshConversation()
      if (staleMessage) {
        setFailure(staleMessage)
        await refreshQueue(sourceTypeFromFocusKey(options.focusKey))
      } else if (commandResult?.kind === 'proposal' && commandResult.actionId && options.focusKey) {
        const item = items.find((candidate) => candidate.key === options.focusKey)
        if (item) await performAction(item, commandResult.actionId, commandResult.payload, { commandDecisionId: commandResult.decisionId, replaceBusy: true })
      }
    } catch (error) {
      if (!controller.signal.aborted) setFailure(error instanceof Error ? error.message : 'Scout could not process that instruction.')
    } finally {
      if (commandAbortRef.current === controller) commandAbortRef.current = null
      setStreaming(null)
      endBusy('command')
    }
  }

  type ActivityEntry = Extract<InboxTimelineEntry, { kind: 'activity' }>
  const sourceTypeFor = (entry: ActivityEntry): FocusItem['sourceType'] | undefined =>
    entry.focus.sourceType ?? sourceTypeFromFocusKey(entry.focus.key)

  const confirmTimeline = async (entry: ActivityEntry) => {
    if (!entry.actionId || !entry.confirmationToken || !beginBusy('confirm')) return
    try {
      const result = await runInboxFocusAction({
        key: entry.focus.key,
        actionId: entry.actionId,
        decisionId: entry.decisionId,
        confirmationToken: entry.confirmationToken,
      })
      await refreshConversation()
      await receiveResult(sourceTypeFor(entry), result)
    } catch (error) {
      setFailure(error instanceof Error ? `Confirmation outcome could not be verified: ${error.message}. Retry to recover the persisted result.` : 'Confirmation outcome could not be verified.')
    } finally {
      endBusy('confirm')
    }
  }

  const cancelTimeline = async (entry: ActivityEntry) => {
    if (!beginBusy('cancel')) return
    try {
      const result = await runInboxFocusAction({ cancelDecisionId: entry.decisionId })
      if (result.status === 'stale') setFailure(result.message ?? 'That confirmation is no longer pending.')
      await refreshConversation()
    } catch (error) {
      setFailure(error instanceof Error ? error.message : 'The confirmation could not be cancelled.')
    } finally {
      endBusy('cancel')
    }
  }

  const undoTimeline = async (entry: ActivityEntry) => {
    if (!beginBusy('undo')) return
    try {
      const result = await runInboxFocusAction({ undoDecisionId: entry.decisionId })
      if (result.status === 'failed' || result.status === 'stale') setFailure(result.message ?? 'That action could not be undone.')
      else setFailure(null)
      await Promise.all([refreshConversation(), refreshQueue(sourceTypeFor(entry))])
    } finally {
      endBusy('undo')
    }
  }

  const retryTimeline = (entry: ActivityEntry) => {
    const item = items.find((candidate) => candidate.key === entry.focus.key)
    if (!item || !entry.actionId) {
      setFailure('That source changed. Open it to review the current state.')
      return
    }
    void performAction(item, entry.actionId)
  }

  const contextValue = useMemo<InboxFocusWorkspaceValue>(() => ({
    data,
    isLoading,
    isError,
    refetch,
    orderedItems,
    active,
    recommendedAction,
    busyAction,
    snoozeMs,
    setSnoozeMs,
    performAction,
    snooze,
    skip,
  }), [active, busyAction, data, isError, isLoading, orderedItems, performAction, recommendedAction, refetch, skip, snooze, snoozeMs])

  return (
    <InboxFocusWorkspaceContext.Provider value={contextValue}>
      <div className="relative flex h-full min-h-0 overflow-hidden bg-surface">
        <InboxChatPanel
          ref={panelRef}
          active={attachActiveDecision ? active : null}
          focusMode={attachActiveDecision}
          assistant={data?.assistant}
          busy={busyAction !== null}
          notice={failure ?? assistantMessage}
          streaming={streaming}
          onSubmit={(instruction, options) => void submitCommand(instruction, options)}
          onConfirm={(entry) => void confirmTimeline(entry)}
          onCancel={(entry) => void cancelTimeline(entry)}
          onRetry={retryTimeline}
          onUndo={(entry) => void undoTimeline(entry)}
        />
        <div className="min-h-0 min-w-0 flex-1 overflow-hidden">{children}</div>
      </div>
    </InboxFocusWorkspaceContext.Provider>
  )
}
