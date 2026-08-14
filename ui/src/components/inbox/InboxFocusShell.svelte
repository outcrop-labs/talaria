<script lang="ts">
  import { setContext, untrack } from 'svelte'
  import type { Snippet } from 'svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import InboxChatPanel from '@/components/inbox/InboxChatPanel.svelte'
  import type { InboxChatPanelHandle, InboxCommandOptions, StreamingTurn } from '@/components/inbox/inbox-chat-panel'
  import {
    runInboxFocusAction,
    streamInboxFocusCommand,
    updateInboxFocusState,
    useInboxFocus,
    type FocusActionResult,
    type FocusItem,
    type InboxTimelineEntry,
  } from '@/lib/inbox-focus.svelte'
  import {
    INBOX_FOCUS_WORKSPACE_KEY,
    INBOX_SNOOZE_OPTIONS,
    type InboxFocusWorkspaceValue,
  } from './inbox-focus-shell'
  import type { AssistantSurface } from '@/lib/inbox-focus-surface'

  function sourceTypeFromFocusKey(focusKey: string | null | undefined): FocusItem['sourceType'] | undefined {
    return focusKey?.split(':', 1)[0] as FocusItem['sourceType'] | undefined
  }

  let {
    children,
    attachActiveDecision,
    surface,
  }: { children: Snippet; attachActiveDecision: boolean; surface: AssistantSurface } = $props()

  const queryClient = useQueryClient()
  const focusQuery = useInboxFocus()
  let skippedKeys = $state<string[]>([])
  let snoozeMs = $state<number>(INBOX_SNOOZE_OPTIONS[0].value)
  // $state is synchronous, so the single variable does what React needed a
  // busyRef mirror for (reads inside async handlers always see the latest).
  let busyAction = $state<string | null>(null)
  let failure = $state<string | null>(null)
  let assistantMessage = $state<string | null>(null)
  let streaming = $state<StreamingTurn | null>(null)
  let panel = $state<InboxChatPanelHandle | null>(null)
  let commandAbort: AbortController | null = null

  $effect(() => () => commandAbort?.abort())

  function beginBusy(action: string, replace = false): boolean {
    if (busyAction && !replace) return false
    busyAction = action
    return true
  }

  function endBusy(action: string) {
    if (busyAction !== action) return
    busyAction = null
  }

  const items = $derived(focusQuery.data?.items ?? [])
  // Drop skipped keys that left the queue. `untrack` keeps the effect keyed on
  // `items` alone (mirroring the React deps), and the length check makes the
  // write conditional so the effect settles instead of looping.
  $effect(() => {
    const keySet = new Set(items.map((item) => item.key))
    untrack(() => {
      const next = skippedKeys.filter((key) => keySet.has(key))
      if (next.length !== skippedKeys.length) skippedKeys = next
    })
  })

  const orderedItems = $derived.by(() => {
    const skipped = new Set(skippedKeys)
    const byKey = new Map(items.map((item) => [item.key, item]))
    return [...items.filter((item) => !skipped.has(item.key)), ...skippedKeys.flatMap((key) => byKey.get(key) ?? [])]
  })
  const active = $derived(orderedItems[0] ?? null)
  const recommendedAction = $derived(active?.actions.find((action) => action.id === active.recommendedActionId) ?? active?.actions[0] ?? null)

  // Keyed on `active?.key` (not the object) like the React deps were, so a
  // background poll that rebuilds the queue with the same item does not re-PUT
  // `viewed` every 30 seconds.
  let viewedKey: string | null = null
  $effect(() => {
    const item = active
    if (!item || !attachActiveDecision) {
      viewedKey = null
      return
    }
    if (viewedKey === item.key) return
    viewedKey = item.key
    void updateInboxFocusState({ sourceType: item.sourceType, sourceId: item.sourceId, viewed: true }).catch(() => {})
  })

  async function refreshQueue(sourceType?: FocusItem['sourceType']) {
    const invalidations = [
      focusQuery.refetch(),
      queryClient.invalidateQueries({ queryKey: ['inbox-focus-summary'] }),
    ]
    if (sourceType === 'task') invalidations.push(queryClient.invalidateQueries({ queryKey: ['home'] }))
    if (sourceType === 'notification') invalidations.push(queryClient.invalidateQueries({ queryKey: ['notifications'] }))
    if (sourceType === 'channel') invalidations.push(queryClient.invalidateQueries({ queryKey: ['channels'] }))
    if (sourceType === 'approval') invalidations.push(queryClient.invalidateQueries({ queryKey: ['google-pending'] }))
    await Promise.all(invalidations)
  }

  const refreshConversation = () => queryClient.invalidateQueries({ queryKey: ['inbox-focus-conversation'] })

  async function receiveResult(sourceType: FocusItem['sourceType'] | undefined, result: FocusActionResult) {
    if (result.status === 'failed') {
      failure = result.message ?? 'That action did not complete.'
      panel?.expand()
      return
    }
    if (result.status === 'stale') {
      failure = result.message ?? 'That item changed. The queue has been refreshed.'
      await refreshQueue(sourceType)
      return
    }
    if (result.status === 'completed') {
      failure = null
      assistantMessage = null
      await refreshQueue(sourceType)
    }
  }

  async function performAction(
    item: FocusItem,
    actionId: string,
    payload?: unknown,
    options: { commandDecisionId?: string; replaceBusy?: boolean } = {},
  ) {
    if (actionId === 'reply' && payload === undefined) {
      assistantMessage = 'Write the reply below. Your assistant will show the exact message for confirmation before posting it.'
      panel?.expand()
      panel?.insertText('Reply: ')
      return
    }
    if (!beginBusy(actionId, options.replaceBusy)) return
    failure = null
    try {
      const result = await runInboxFocusAction({
        key: item.key,
        actionId,
        payload,
        commandDecisionId: options.commandDecisionId,
      })
      if (result.status === 'confirmation_required') panel?.expand()
      else await receiveResult(item.sourceType, result)
      await refreshConversation()
    } catch (error) {
      failure = error instanceof Error ? error.message : 'The action failed.'
    } finally {
      endBusy(actionId)
    }
  }

  async function snooze() {
    if (!active || !beginBusy('snooze')) return
    try {
      const result = await updateInboxFocusState({
        sourceType: active.sourceType,
        sourceId: active.sourceId,
        snoozedUntil: new Date(Date.now() + snoozeMs).toISOString(),
      })
      assistantMessage = `Snoozed for ${INBOX_SNOOZE_OPTIONS.find((option) => option.value === snoozeMs)?.label.toLowerCase()}.`
      if (result.timelineEntry) await refreshConversation()
      await refreshQueue()
    } catch (error) {
      failure = error instanceof Error ? error.message : 'The item could not be snoozed.'
    } finally {
      endBusy('snooze')
    }
  }

  function skip() {
    if (!active || orderedItems.length < 2 || busyAction) return
    skippedKeys = [...skippedKeys.filter((key) => key !== active.key), active.key]
    failure = null
    assistantMessage = null
  }

  async function submitCommand(instruction: string, options: InboxCommandOptions) {
    const trimmedInstruction = instruction.trim()
    if (!trimmedInstruction || !beginBusy('command')) return
    failure = null
    assistantMessage = null
    streaming = {
      user: instruction,
      status: options.mode === 'plan' ? 'Planning with your assistant' : options.mode === 'fast' ? 'Answering quickly' : options.focusKey ? 'Reviewing the active decision' : 'Thinking with your assistant',
      content: '',
    }
    const controller = new AbortController()
    commandAbort = controller
    let commandResult: Exclude<import('@/lib/inbox-focus.svelte').FocusCommandResponse, { status: 'stale' }> | null = null
    let staleMessage: string | null = null
    try {
      for await (const event of streamInboxFocusCommand({
        key: options.focusKey,
        surface: surface.id,
        instruction: trimmedInstruction,
        delegateModel: options.delegateModel,
        responseModel: options.responseModel,
        mode: options.mode,
        attachmentIds: options.attachmentIds,
        refs: options.refs,
      }, controller.signal)) {
        if (event.type === 'status' && streaming) streaming.status = event.label
        if (event.type === 'content' && streaming) streaming.content += event.text
        if (event.type === 'error') throw new Error(event.message)
        if (event.type === 'done' && event.result) {
          if ('status' in event.result) staleMessage = event.result.message
          else commandResult = event.result
        }
      }
      await refreshConversation()
      if (staleMessage) {
        failure = staleMessage
        await refreshQueue(sourceTypeFromFocusKey(options.focusKey))
      } else if (commandResult?.kind === 'proposal' && commandResult.actionId && options.focusKey) {
        // Handing the proposal to the server is NOT the same as executing it,
        // and this line must not be read as if it were. `runFocusAction` looks
        // up who proposed the action (`requiresHumanConfirmation`), and answers
        // `confirmation_required` for anything a model selected rather than a
        // regex matched — `performAction` then just expands the panel and the
        // owner clicks Confirm on the timeline card. The decision lives on the
        // server because only the server knows the proposal's provenance; the
        // client cannot tell them apart and must not try.
        const item = items.find((candidate) => candidate.key === options.focusKey)
        if (item) await performAction(item, commandResult.actionId, commandResult.payload, { commandDecisionId: commandResult.decisionId, replaceBusy: true })
      }
    } catch (error) {
      if (!controller.signal.aborted) failure = error instanceof Error ? error.message : 'Your assistant could not process that instruction.'
    } finally {
      if (commandAbort === controller) commandAbort = null
      streaming = null
      endBusy('command')
    }
  }

  type ActivityEntry = Extract<InboxTimelineEntry, { kind: 'activity' }>
  const sourceTypeFor = (entry: ActivityEntry): FocusItem['sourceType'] | undefined =>
    entry.focus.sourceType ?? sourceTypeFromFocusKey(entry.focus.key)

  async function confirmTimeline(entry: ActivityEntry) {
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
      failure = error instanceof Error ? `Confirmation outcome could not be verified: ${error.message}. Retry to recover the persisted result.` : 'Confirmation outcome could not be verified.'
    } finally {
      endBusy('confirm')
    }
  }

  async function cancelTimeline(entry: ActivityEntry) {
    if (!beginBusy('cancel')) return
    try {
      const result = await runInboxFocusAction({ cancelDecisionId: entry.decisionId })
      if (result.status === 'stale') failure = result.message ?? 'That confirmation is no longer pending.'
      await refreshConversation()
    } catch (error) {
      failure = error instanceof Error ? error.message : 'The confirmation could not be cancelled.'
    } finally {
      endBusy('cancel')
    }
  }

  async function undoTimeline(entry: ActivityEntry) {
    if (!beginBusy('undo')) return
    try {
      const result = await runInboxFocusAction({ undoDecisionId: entry.decisionId })
      if (result.status === 'failed' || result.status === 'stale') failure = result.message ?? 'That action could not be undone.'
      else failure = null
      await Promise.all([refreshConversation(), refreshQueue(sourceTypeFor(entry))])
    } finally {
      endBusy('undo')
    }
  }

  function retryTimeline(entry: ActivityEntry) {
    const item = items.find((candidate) => candidate.key === entry.focus.key)
    if (!item || !entry.actionId) {
      failure = 'That source changed. Open it to review the current state.'
      return
    }
    void performAction(item, entry.actionId)
  }

  // The context value the workspace surfaces read (FocusInbox et al). Getters
  // keep every field reactive — consumers must not destructure it.
  setContext<InboxFocusWorkspaceValue>(INBOX_FOCUS_WORKSPACE_KEY, {
    get data() {
      return focusQuery.data
    },
    get isLoading() {
      return focusQuery.isLoading
    },
    get isError() {
      return focusQuery.isError
    },
    get error() {
      return focusQuery.error
    },
    refetch: () => focusQuery.refetch(),
    get orderedItems() {
      return orderedItems
    },
    get active() {
      return active
    },
    get recommendedAction() {
      return recommendedAction
    },
    get busyAction() {
      return busyAction
    },
    get snoozeMs() {
      return snoozeMs
    },
    setSnoozeMs: (value: number) => {
      snoozeMs = value
    },
    performAction,
    snooze,
    skip,
  })
</script>

<div class="relative flex h-full min-h-0 min-w-0 flex-1 overflow-hidden bg-surface">
  <InboxChatPanel
    bind:this={panel}
    active={attachActiveDecision ? active : null}
    focusMode={attachActiveDecision}
    surfaceLabel={surface.label}
    assistant={focusQuery.data?.assistant}
    busy={busyAction !== null}
    notice={failure ?? assistantMessage}
    {streaming}
    onSubmit={(instruction, options) => void submitCommand(instruction, options)}
    onConfirm={(entry) => void confirmTimeline(entry)}
    onCancel={(entry) => void cancelTimeline(entry)}
    onRetry={retryTimeline}
    onUndo={(entry) => void undoTimeline(entry)}
  />
  <!-- A COLUMN, not a plain block. This slot used to hold one page element that
       sized itself with `h-full`, so a block was enough. It now holds the top
       strip, the banner and the page stacked vertically, and a block parent
       gives its child `height: auto` — every `flex-1 min-h-0` scroll region
       below here then measures against nothing, grows to its content, and the
       page stops scrolling. The height chain has to be unbroken from `h-screen`
       all the way down. -->
  <div class="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden">{@render children()}</div>
</div>
