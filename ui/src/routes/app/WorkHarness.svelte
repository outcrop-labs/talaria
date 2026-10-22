<script lang="ts">
  import { untrack } from 'svelte'
  import { Ellipsis, Files, MessageSquare, Plus, Telescope, X } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { navigate, route } from '@/router'
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import { fade } from '@/lib/motion'
  import { legacyRunIdFor, sessionIdFor } from '@/lib/work-selection'
  import { claimViewTitle } from '@/lib/view-title.svelte'
  import ChatView from '@/components/chat/ChatView.svelte'
  import ComposerPicker from '@/components/chat/ComposerPicker.svelte'
  import ConversationMembers from '@/components/chat/ConversationMembers.svelte'
  import ConversationSidebar from '@/components/chat/ConversationSidebar.svelte'
  import type { SidebarFailure } from '@/components/chat/conversation-sidebar'
  import { userMentionInsert } from '@/components/chat/mentions.svelte'
  import PlanDocSkeleton from '@/components/chat/PlanDocSkeleton.svelte'
  import { planDraft } from '@/components/chat/plan-drafts.svelte'
  import RailSurface from '@/components/app/RailSurface.svelte'
  import Stage from '@/components/app/Stage.svelte'
  import StageHeader from '@/components/app/StageHeader.svelte'
  import { planRowMenu, researchRowMenu } from '@/components/record-menus'
  import NoModelBump from '@/components/setup/NoModelBump.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { alert, prompt } from '@/components/ui/confirm.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import DraftTicketsButton from '@/components/work/DraftTicketsButton.svelte'
  import ExportButton from '@/components/work/ExportButton.svelte'
  import FileGlyph from '@/components/work/FileGlyph.svelte'
  import FilePane from '@/components/work/FilePane.svelte'
  import LivingDocPane from '@/components/work/LivingDocPane.svelte'
  import ReportPane from '@/components/work/ReportPane.svelte'
  import ResearchComposer from '@/components/work/ResearchComposer.svelte'
  import ScheduleButton from '@/components/work/ScheduleButton.svelte'
  import SessionPalette from '@/components/work/SessionPalette.svelte'
  import StarterCards from '@/components/work/StarterCards.svelte'
  import { sendsSeed, type StarterCard } from '@/components/work/starter-cards'
  import WorkspacePane from '@/components/work/WorkspacePane.svelte'
  import { useAgents } from '@/lib/agents'
  import { createArtifact } from '@/lib/artifacts'
  import { streamChat, type ChatFocus } from '@/lib/chat'
  import { toastError } from '@/lib/toast.svelte'
  import {
    archiveConversation,
    deleteConversation,
    loadConversation,
    renameConversation,
    restoreConversation,
    setPinnedFiles,
    useConversationMembers,
    useConversations,
    type Conversation,
    type PinnedFile,
  } from '@/lib/conversations.svelte'
  import { parseGoogleFileUrl } from '@/lib/google-embed'
  import {
    appendPin,
    changedFiles,
    fileGlyph,
    fileTitle,
    focusTarget,
    hiddenFiles,
    hideFile,
    initialFocus,
    resurfaced,
    unhideFile,
    useSessionFiles,
    visibleFiles,
    type SessionFile,
  } from '@/lib/session-files'
  import {
    MODE_META,
    openResearchConversation,
    removeResearch,
    renameResearch,
    researchLabel,
    useResearchMembers,
    useResearchRun,
    useResearchRuns,
    type ResearchRun,
  } from '@/lib/research'
  import { useHasPerm, useSession } from '@/lib/session'
  import { useStickyAgent } from '@/lib/sticky-agent.svelte'
  import { useMarkResearchRunSeen } from '@/lib/unreads.svelte'
  import {
    closeTab,
    openTab,
    seedTabs,
    selectTab,
    syncTouched,
    tabsFor,
    type WorkspaceTab,
  } from '@/lib/workspace-tabs.svelte'
  import ResearchMembers from './research/ResearchMembers.svelte'

  // THE WORK HARNESS — one surface for every session you work in.
  //
  // Work, Plan and Research were three views of the same shape: a rail of
  // sessions, a stream on the left, and the thing the session is about on the
  // right. The differences were real but small — a plan drafts tickets, a
  // research run carries a cited report and a question it may be parked on, a
  // work session writes to Google behind your approval — and keeping them as
  // three views meant three rails, three agent pickers, three empty states and
  // three places to fix anything that was wrong with all of them. It also made
  // the person do the filing: "is this thing I am thinking about a plan or a
  // session?" is a question the tool should never ask.
  //
  // So: ONE rail over `?kind=sessions` (work + plan + research, one query, one
  // ordering — see `list_conversations_in`), the stream on the LEFT with the
  // session's own kind, and the workspace on the RIGHT as a strip of tabs: the
  // session's FILES (every document, sheet, page and Google file the agent
  // created or opened in it, and anything a teammate opened there —
  // `lib/session-files`), plus the run's report and the tickets it has drafted.
  // A PLAN session also keeps its living plan document as the first tab; a work
  // session has no such document and none of its vocabulary. The tabs are
  // `lib/workspace-tabs`; each tab's body is a pane out of `components/work/`.
  //
  // THE OLD ADDRESSES STILL RESOLVE. `/plan/<id>` is a session id and reads as
  // the selection; `/research/<runId>` resolves the run's own discussion and
  // replaces itself with `/work/<conversationId>`, falling back to the report
  // alone for a run that has no discussion to open (see `standaloneRunId`). A
  // URL somebody mailed is a promise.

  // The run's state as a dot. Parked on a question is accent WITHOUT the pulse
  // — "waiting on you", not "working". The pulse stays running-only so those
  // two never blur.
  const STATUS_DOT: Record<ResearchRun['status'], DotStatus> = {
    queued: 'idle',
    running: 'accent',
    awaiting: 'accent',
    done: 'ok',
    error: 'danger',
  }

  const qc = useQueryClient()
  const mayManageAgents = useHasPerm('agents.manage')
  const sessionQuery = useSession()
  const me = $derived(sessionQuery.data)
  // Both reads keep their query object: the rail and the stage each render a
  // sentence ("No sessions yet.", "No agents available.") that
  // is only true of a request that SUCCEEDED and came back empty.
  const fleetQuery = useAgents()
  const agentsLoading = $derived(fleetQuery.isLoading)
  const agents = $derived(fleetQuery.data?.agents ?? [])
  const conversationsQuery = useConversations('sessions')
  const conversations = $derived(conversationsQuery.data ?? [])
  const archivedQuery = useConversations('sessions', true)
  const archived = $derived(archivedQuery.data ?? [])
  const conversationsLoading = $derived(conversationsQuery.isLoading)
  // Stale beats blank: a failed BACKGROUND refetch still has good data to show,
  // so only a failure with nothing behind it becomes a visible failure.
  const agentsFailure: SidebarFailure = $derived(
    fleetQuery.isError && fleetQuery.data === undefined
      ? { error: fleetQuery.error, retry: () => void fleetQuery.refetch() }
      : null,
  )
  const conversationsFailure: SidebarFailure = $derived(
    conversationsQuery.isError && conversationsQuery.data === undefined
      ? { error: conversationsQuery.error, retry: () => void conversationsQuery.refetch() }
      : null,
  )
  const archivedFailure: SidebarFailure = $derived(
    archivedQuery.isError && archivedQuery.data === undefined
      ? { error: archivedQuery.error, retry: () => void archivedQuery.refetch() }
      : null,
  )

  // THE URL IS THE SESSION SELECTION (/work/<id>) — linkable, back/forward-able.
  // `/plan/<id>` is read too: a plan is a session now, and its old address
  // names the same row.
  //
  // DECLARED ABOVE ITS FIRST READER, and the position is load-bearing.
  // createQuery resolves its options function EAGERLY to seed the observer
  // (createBaseQuery.svelte.js: `new Observer(client, resolvedOptions)`), so a
  // `() => selectedSessionId` getter below is called during this component's
  // init rather than on first read. Declared after that call, the `const` was
  // read inside its own temporal dead zone and the view threw on every mount.
  const selectedSessionId = $derived(sessionIdFor(route.pathname))

  // ── A RUN'S OLD ADDRESS ────────────────────────────────────────────────────
  //
  // /research/<runId> was the run's page. The run's discussion is a session
  // now, so the address resolves to it — one detail read, which is the same
  // query key the report pane uses, so nothing is fetched twice.
  const legacyRunId = $derived(legacyRunIdFor(route.pathname))
  const legacyRunQuery = useResearchRun(() => legacyRunId)
  const legacyRun = $derived(legacyRunQuery.data?.run ?? null)
  $effect(() => {
    const conversationId = legacyRunId ? legacyRun?.conversationId : null
    // `replace`, so the back button goes where the person came from rather
    // than bouncing them back through the address they just left.
    if (conversationId) void navigate('/work/:sessionId', { params: { sessionId: conversationId }, replace: true })
  })
  // A run an ownerless org agent started has nobody to own a thread, and rows
  // that predate eager conversations have none either: the report IS the
  // surface, and the way into a discussion is offered in the header.
  const standaloneRunId = $derived(legacyRunId && legacyRun && !legacyRun.conversationId ? legacyRunId : null)

  // WHICH SESSIONS ARE RESEARCH RUNS. The merged list carries no kind on the
  // wire, and the runs list — which this surface wants anyway, for the report
  // tab and the run's own state — answers the question by naming each run's
  // conversation. One read, no new endpoint.
  const runsQuery = useResearchRuns()
  const runBySession = $derived(
    new Map((runsQuery.data ?? []).filter((r) => r.conversationId).map((r) => [r.conversationId!, r])),
  )
  const selectedRun = $derived(selectedSessionId ? (runBySession.get(selectedSessionId) ?? null) : null)
  const reportRunId = $derived(selectedRun?.id ?? null)
  const canManageRun = (run: ResearchRun) => run.ownerUserId === me?.id || me?.role === 'admin'
  // RUNS WITH NO THREAD — the other half of that same list, and the half a
  // rail of CONVERSATIONS cannot show: an ownerless org agent's run has nobody
  // to own a discussion, and rows that predate eager conversations have none
  // either. They are still somebody's research, so they get a group of their
  // own wherever this surface has room to list it, each opening the standalone
  // report at its own address.
  const orphanRuns = $derived((runsQuery.data ?? []).filter((r) => !r.conversationId))

  // OPENING IS THE SEEN GESTURE for research — the same walk-in a thread's read
  // cursor performs. The notifications that pointed here (the run's landing,
  // its questions) clear by href, and the rail badge falls with the bell. Fired
  // once per run, whichever way the run was reached.
  const markSeen = useMarkResearchRunSeen()
  let seenRun: string | null = null
  $effect(() => {
    const runId = selectedRun?.id ?? standaloneRunId
    if (!runId || seenRun === runId) return
    seenRun = runId
    void markSeen(runId)
  })

  // @mention the people a mention will actually REACH, which is a different
  // list per kind of session: a plan or a work session shares through
  // `conversation_members`, a research run through the run's own members (the
  // list that decides who may read the report). Each read is gated on the
  // session being the kind that answers it — asking the conversation plane
  // about a research thread is a 404, since `shared_conversation_role` answers
  // for plans and work sessions only.
  const membersQuery = useConversationMembers(() => (selectedRun ? null : selectedSessionId))
  const runMembersQuery = useResearchMembers(() => selectedRun?.id ?? null)
  const mentionables = $derived(
    ((selectedRun ? runMembersQuery.data?.members : membersQuery.data?.members) ?? [])
      .map((u) => ({
        insert: userMentionInsert({ name: u.name, email: u.email }),
        label: u.name ?? u.email ?? u.userId,
        sub: u.email ?? undefined,
      }))
      // Somebody with neither a name nor an email cannot be mentioned in a way
      // the notifier can resolve, so offering them would be an @ going nowhere.
      .filter((m) => m.insert),
  )

  const sticky = useStickyAgent('work', () => agents)
  const selectedAgent = $derived(sticky.selected)
  const pickAgent = sticky.select
  const setSelectedSessionId = (id: string | null, opts: { replace?: boolean } = {}) => {
    if (id) void navigate('/work/:sessionId', { params: { sessionId: id }, replace: opts.replace })
    else void navigate('/work', { replace: opts.replace })
  }
  let newChatSignal = $state(0)
  // The model-tier pick lives in the VIEW, not the composer: a session's chat
  // surface is attach + text + submit only, and the rail owns the agent. A new
  // session starts on the agent's main model ('').
  let sessionTier = $state('')
  // NO TEMPLATE PICKER. It stood on the zero state to choose what a new
  // session's living document seeded from — and a work session has no living
  // document to seed. A plan started from its starter card seeds from the
  // agent's bound plan template server-side, which is what "Automatic" was.
  //
  // Bumped when an agent turn lands; a PLAN's living document asks for its new
  // body on the bump. The server also rewrites a plan's document on a landed
  // turn, and its recency guard means whichever gets there first does the one
  // rewrite.
  let turnSignal = $state(0)
  // The run's own posts — the scope questions, the ack, the report-ready turn —
  // land in the discussion without going through ChatView's pollers, and the
  // runs query outlives that view's resuming cap. Every status transition
  // nudges the chat to re-read: scope→awaiting (the questions), awaiting→
  // running (the ack), running→done (the report-ready turn).
  let lastRunStatus: string | null = null
  let chatSyncSignal = $state(0)
  $effect(() => {
    const stamp = selectedRun ? `${selectedRun.id}:${selectedRun.status}` : null
    if (!stamp || stamp === lastRunStatus) return
    lastRunStatus = stamp
    chatSyncSignal += 1
  })

  // ── THE NARROW READING ─────────────────────────────────────────────────────
  //
  // Below `lg` the stage has room for ONE column, and which one it should be is
  // a question about the session. A work session's stream IS the work, so it
  // keeps the screen. A research run is the other way round — RunView dropped
  // the discussion entirely below that breakpoint, because a phone is for
  // reading the answer — so a research-origin session lands on its document.
  //
  // It is a DEFAULT and not a rule: the toggle in the header (narrow screens
  // only; above `lg` both columns are up and there is nothing to choose) is how
  // somebody says otherwise, and the pick then stands until they open another
  // session.
  let narrowView = $state<'stream' | 'document'>('stream')
  let narrowFor: string | null = null
  $effect(() => {
    const id = selectedSessionId
    if (!id) {
      // The zero state is a composer: there is no document to default to.
      narrowFor = null
      narrowView = 'stream'
      return
    }
    // Waits for the runs list for the same reason the strip does — until it
    // lands we do not know which kind of session this is.
    if (runsQuery.isLoading || narrowFor === id) return
    narrowFor = id
    narrowView = runBySession.has(id) ? 'document' : 'stream'
  })

  // ── THE WORKSPACE STRIP ────────────────────────────────────────────────────
  //
  // The tabs are the session's, held in `lib/workspace-tabs` and fed from here.
  // What a session HAS is a question the server answers — `GET
  // /api/conversations/{id}/files`, the documents, sheets, pages and Google
  // files its messages name plus the ones pinned to it — and the strip is that
  // answer, plus the two tabs that are not files: the run's report and the
  // drafts waiting on a person. Nothing is created because a session was
  // opened.
  const PLAN_TAB: WorkspaceTab = { id: 'doc', kind: 'doc', label: 'Plan', glyph: 'plan' }
  const REPORT_TAB = 'report'
  const DRAFTS_TAB = 'drafts'
  const strip = $derived(tabsFor(selectedSessionId))

  const filesQuery = useSessionFiles(() => selectedSessionId)
  // `undefined` while the first read is in flight — or while the result still
  // answers the session that was open a moment ago, which is never this one's;
  // `null` is the route's 404 (a session this viewer cannot read), which has
  // nothing to list.
  const filesAnswer = $derived(
    filesQuery.data && filesQuery.data.sessionId === selectedSessionId ? filesQuery.data.answer : undefined,
  )
  const filesPending = $derived(filesAnswer === undefined && !filesQuery.isError)
  const filesFailed = $derived(filesAnswer === undefined && filesQuery.isError)
  // WHICH PANE THIS SESSION GETS: a plan keeps its living plan document, a
  // research run its report, and everything else — a work session above all —
  // gets its files. A run is known from the runs list (the rail's own read);
  // the rest from the files answer's `kind`. Null until one of them has said,
  // and the pane holds its skeleton until then.
  const paneKind = $derived<string | null>(
    selectedRun ? 'research' : filesAnswer === undefined ? null : (filesAnswer?.kind ?? 'work'),
  )
  // THE RUN'S REPORT IS ALSO A FILE the session touched — the report-ready turn
  // links it — and it already has a tab, with its sources and the question it
  // is parked on. So it is that tab, never a second one onto the same document.
  const reportKey = $derived(selectedRun?.artifactId ? `artifact:${selectedRun.artifactId}` : null)
  const sessionFiles = $derived((filesAnswer?.files ?? []).filter((f) => f.key !== reportKey))
  const fileByKey = $derived(new Map(sessionFiles.map((f) => [f.key, f])))
  const fileTab = (f: SessionFile): WorkspaceTab => ({
    id: f.key,
    kind: f.kind,
    label: fileTitle(f),
    ref: f.id,
    glyph: fileGlyph(f),
  })

  // WHAT THE LAST READ SAID, per session: the baseline a refetch is compared
  // with to find what is new. Plain rather than `$state` — it is bookkeeping
  // the effect below writes, and writing it must not re-run anything.
  const lastFiles = new Map<string, SessionFile[]>()
  const applyFiles = (id: string, kind: string, all: SessionFile[]) => {
    // A file this viewer closed comes back by itself once a newer message
    // touches it — compared with its touch time WHEN it was closed, so this
    // holds across a reload as well as within one visit.
    for (const key of resurfaced(hiddenFiles(id), all)) unhideFile(id, key)
    const prev = lastFiles.get(id) ?? null
    lastFiles.set(id, all)
    const files = all.filter((f) => f.key !== reportKey)
    const visible = visibleFiles(files, hiddenFiles(id))
    if (prev === null) {
      // FIRST SIGHT in this page's life, so nothing is news yet: the strip
      // takes the files in the server's order and lands on the tab this viewer
      // was reading last time, else the file the agent touched most recently,
      // else the first (`seedTabs` defers to the store's memory). A plan leads
      // with its own document and lands there.
      const lead = kind === 'plan' ? [PLAN_TAB] : []
      seedTabs(id, [...lead, ...visible.map(fileTab)], kind === 'plan' ? PLAN_TAB.id : initialFocus(visible))
      return
    }
    // A file the session no longer lists — deleted, or no longer readable —
    // leaves the strip: a tab onto a 404 is not a way back to anything.
    const listed = new Set(files.map((f) => f.key))
    // A copy: `closeTab` splices the very array being walked.
    for (const tab of [...tabsFor(id).tabs]) {
      if ((tab.kind === 'artifact' || tab.kind === 'google') && !listed.has(tab.id)) closeTab(id, tab.id)
    }
    // THE PANE FOLLOWS THE AGENT: a file listed for the first time, or touched
    // again, comes back if it was closed and takes the pane — the newest touch
    // wins when a turn touched several.
    const changed = changedFiles(prev, all)
    for (const f of changed) unhideFile(id, f.key)
    // Every visible file has a tab (titles refreshed in place); none of this
    // moves anybody's reading.
    seedTabs(id, visibleFiles(files, hiddenFiles(id)).map(fileTab))
    const target = focusTarget(changed)
    if (!target) return
    if (target.key === reportKey) selectTab(id, REPORT_TAB)
    else syncTouched(id, fileTab(target))
  }
  $effect(() => {
    const id = selectedSessionId
    const answer = filesAnswer
    const kind = paneKind
    // Waits for the runs list as well: until it lands, a research run's report
    // would be listed as an ordinary file.
    if (!id || answer === undefined || kind === null || runsQuery.isLoading) return
    const all = answer?.files ?? []
    untrack(() => applyFiles(id, kind, all))
  })
  // A PLAN'S DOCUMENT RE-OPENS ITSELF when the strip empties. It closes like
  // any tab — closing it with a file open beside it is a person saying "not
  // now" — but a plan with nothing on its strip has lost the thing it is for.
  $effect(() => {
    const id = selectedSessionId
    if (id && paneKind === 'plan' && strip.tabs.length === 0) openTab(id, PLAN_TAB)
  })
  // THE REPORT IS A TAB, from the moment the run is known rather than from the
  // moment it finishes: the pane renders where the run has got to and the
  // question it is parked on, which is the half that matters while it works.
  //
  // RE-OPENED, not merely seeded: a research session's report IS its document,
  // so an empty strip is a dead end rather than a choice. Closing it with
  // another tab open is a person saying "not now", and that is honoured.
  const reported = new Set<string>()
  $effect(() => {
    const id = selectedSessionId
    const run = selectedRun
    if (!id || !run || (reported.has(id) && strip.tabs.length > 0)) return
    reported.add(id)
    openTab(id, { id: REPORT_TAB, kind: 'report', label: 'Report', ref: run.id, glyph: 'report' })
  })
  // DRAFTS WAITING ON A PERSON get a tab, and only then. A job that is still
  // drafting is the agent working, which belongs in the stream and in the
  // header button; a batch that has landed is waiting on somebody, and this
  // surface says so where they are looking.
  const draftJob = $derived(selectedSessionId ? planDraft(selectedSessionId) : undefined)
  const draftsOpened = new Set<string>()
  $effect(() => {
    const id = selectedSessionId
    if (!id || draftJob?.status !== 'ready' || draftsOpened.has(id)) return
    draftsOpened.add(id)
    openTab(id, { id: DRAFTS_TAB, kind: 'drafts', label: 'Drafts', glyph: 'drafts' })
  })
  const draftSentence = $derived(
    draftJob?.status === 'ready'
      ? `${draftJob.proposals.length} ticket${draftJob.proposals.length === 1 ? '' : 's'} drafted from this session, waiting for your review.`
      : draftJob?.status === 'drafting'
        ? 'The agent is reading the session and drafting tickets. It keeps going if you leave.'
        : draftJob?.status === 'failed'
          ? (draftJob.note ?? 'The draft did not come back.')
          : 'Nothing drafted from this session yet.',
  )

  // CLOSING A FILE HIDES IT for this viewer in this session (the file is still
  // the session's, and every other member still sees it). It comes back when
  // the agent touches it again, or from the strip's file list.
  const closeWorkspaceTab = (tab: WorkspaceTab) => {
    const id = selectedSessionId
    if (!id) return
    if (tab.kind === 'artifact' || tab.kind === 'google') {
      hideFile(id, fileByKey.get(tab.id) ?? { key: tab.id, lastTouchedAt: null })
    }
    closeTab(id, tab.id)
  }
  const reopenFile = (f: SessionFile) => {
    const id = selectedSessionId
    if (!id) return
    unhideFile(id, f.key)
    // A person asking for a file is somebody acting, past the last visit's
    // memory too — the same call the agent's touch makes.
    syncTouched(id, fileTab(f))
  }
  // THE FILE LIST, when the strip cannot show everything at a glance: more
  // files than fit, or any this viewer closed.
  const offStrip = $derived(sessionFiles.filter((f) => !strip.tabs.some((t) => t.id === f.key)))
  const showAllFiles = $derived(sessionFiles.length > 6 || offStrip.length > 0)
  const allFilesItems = (): ContextMenuEntry[] =>
    sessionFiles.map((f) => ({
      label: offStrip.includes(f) ? `${fileTitle(f)} (closed)` : fileTitle(f),
      icon: [FileGlyph, { glyph: fileGlyph(f), size: 12 }],
      checked: strip.activeId === f.key,
      onSelect: () => reopenFile(f),
    }))

  // ── OPENING ONE BY HAND ────────────────────────────────────────────────────
  //
  // A teammate can put a file in front of the session too. Either way it is
  // PINNED to the conversation (`pinned_files`), so it is the session's for
  // every member and every reload, and the files route lists it; the refetch
  // finds it new, which is what opens it.
  const refreshFiles = (id: string | null = selectedSessionId) =>
    id ? qc.invalidateQueries({ queryKey: ['session-files', id] }) : Promise.resolve()
  let addingFile = $state(false)
  // APPEND, never replace: opening a second file must not unpin the first.
  // The pins are read fresh so one somebody else added a moment ago survives,
  // and a failed read stops here rather than writing a list missing them.
  const pinToSession = async (id: string, file: PinnedFile): Promise<boolean> => {
    const detail = await loadConversation(id)
    if (!detail) {
      toastError('Could not open that file', new Error('The session did not load. Try again.'))
      return false
    }
    return setPinnedFiles(id, appendPin(detail.conversation.pinnedFiles ?? [], file))
  }
  const newDocument = async () => {
    const id = selectedSessionId
    if (!id || addingFile) return
    addingFile = true
    try {
      const artifact = await createArtifact({ kind: 'doc', title: 'Untitled document' })
      if (await pinToSession(id, { kind: 'artifact', id: artifact.id, title: artifact.title })) await refreshFiles(id)
    } catch (e) {
      toastError('Could not create the document', e)
    } finally {
      addingFile = false
    }
  }
  // PASTE A LINK. The smallest honest way in: people already have the URL. The
  // type is read from the URL's own shape — a link that is not a Google file is
  // refused here rather than pinned into a blank frame.
  const openGoogleFile = async () => {
    const id = selectedSessionId
    if (!id || addingFile) return
    const raw = await prompt({
      title: 'Open a Google file',
      message: 'Paste the link to a Google Doc, Sheet, Slides deck or Drive file.',
      placeholder: 'https://docs.google.com/document/d/…',
      confirmLabel: 'Open',
    })
    if (!raw?.trim()) return
    const hit = parseGoogleFileUrl(raw)
    if (!hit) {
      await alert({
        title: 'Not a Google file link',
        message: 'That did not look like a docs.google.com or drive.google.com file URL.',
      })
      return
    }
    // Already the session's: show it rather than pinning it twice.
    const listed = fileByKey.get(`google:${hit.id}`)
    if (listed) {
      reopenFile(listed)
      return
    }
    addingFile = true
    try {
      const file: PinnedFile = { kind: 'google', id: hit.id, ...(hit.mime ? { mime: hit.mime } : {}) }
      if (await pinToSession(id, file)) await refreshFiles(id)
    } finally {
      addingFile = false
    }
  }
  const openItems = (): ContextMenuEntry[] => [
    { label: 'New document', disabled: addingFile, onSelect: () => void newDocument() },
    { label: 'Open a Google file', disabled: addingFile, onSelect: () => void openGoogleFile() },
  ]

  // THE AGENT IS TOLD WHICH FILE IS OPEN. The active file tab rides along with
  // every work turn (`focus` in the chat body), so "this doc" means the file in
  // front of the teammate. A plan's document, the report and the drafts are
  // not files the agent edits by that name, so they send nothing.
  const activeFile = $derived(
    strip.active && (strip.active.kind === 'artifact' || strip.active.kind === 'google')
      ? (fileByKey.get(strip.active.id) ?? null)
      : null,
  )
  const chatFocus = $derived<ChatFocus | null>(
    activeFile && paneKind !== 'plan'
      ? {
          kind: activeFile.kind,
          id: activeFile.id,
          ...(activeFile.title?.trim() ? { title: activeFile.title.trim() } : {}),
        }
      : null,
  )
  // A plan's living document, resolved by its pane: the id the write-tool
  // invalidation below needs for it.
  let livingDocId = $state<string | null>(null)

  // ── A WRITE DIRTIES A FILE WHETHER ANYTHING IS SHOWING IT ──────────────────
  //
  // Tools whose completion means a file this session is working on changed
  // underneath us, or a new one exists.
  //
  // THE INVALIDATION LIVES HERE, NOT IN THE PANE. It used to be the pane's, and
  // the pane handed its handler up for the stream to call — which meant the
  // whole thing was silently off whenever the pane was not mounted: the report
  // tab or the drafts tab in front, a narrow screen with the stream showing, a
  // session with no workspace column at all. The ids are what the keys are made
  // of, and this view knows them — the strip's artifact tabs plus a plan's
  // living document — so it does the invalidating and the pane just renders.
  //
  // Only the artifact writers re-read an artifact: a Google write of someone
  // else's file queues rather than landing, so re-reading on it would show the
  // unchanged document and imply the write went through. Every writer re-reads
  // the FILE LIST, which is how a file the agent just made gets its tab.
  const MUTATES_ARTIFACT = new Set([
    'create_document',
    'update_document',
    'edit_document',
    'create_sheet',
    'create_page',
    'save_image_artifact',
  ])
  const WRITES_FILE = new Set([
    ...MUTATES_ARTIFACT,
    'create_google_doc',
    'update_google_doc',
    'append_google_doc',
    'import_drive_file',
  ])
  const touchedArtifactIds = $derived([
    ...strip.tabs.filter((t) => t.kind === 'artifact').map((t) => t.ref ?? t.id),
    ...(livingDocId ? [livingDocId] : []),
  ])
  const onToolDone = (name: string) => {
    if (MUTATES_ARTIFACT.has(name)) {
      // Invalidate rather than remount: a pane that IS showing one of these
      // keeps its scroll and any in-progress edit, and TanStack refetches the
      // one key that changed.
      for (const id of touchedArtifactIds) void qc.invalidateQueries({ queryKey: ['artifact', id] })
    }
    if (WRITES_FILE.has(name)) void refreshFiles()
  }
  // A landed turn re-reads the file list (the server names a turn's files from
  // the chips it attaches when the turn is persisted, so this is the read that
  // is sure to see them), and nudges a plan's living document.
  const onTurnComplete = () => {
    turnSignal += 1
    void refreshFiles()
  }

  // ── THE RESEARCH ASK ───────────────────────────────────────────────────────
  //
  // It was the research page's zero state and lived nowhere else. Here it is
  // reachable from any session: ask, and the run opens as a session of its own.
  let researchOpen = $state(false)
  const onResearchStarted = (run: ResearchRun) => {
    researchOpen = false
    // The rail lists conversations, and a run's discussion is one — created
    // with the run, so it is already there to be listed.
    void qc.invalidateQueries({ queryKey: ['conversations'] })
    if (run.conversationId) setSelectedSessionId(run.conversationId)
    // No discussion to open (an ownerless org agent ran it): the report is the
    // thing, at the address that renders it alone.
    else void navigate('/research/:runId', { params: { runId: run.id } })
  }
  let opening = $state(false)
  let openError = $state<string | null>(null)
  const openDiscussion = async (runId: string) => {
    opening = true
    openError = null
    const out = await openResearchConversation(runId)
    opening = false
    if (out.error) openError = out.error
    else if (out.conversationId) {
      void qc.invalidateQueries({ queryKey: ['conversations'] })
      setSelectedSessionId(out.conversationId)
    }
  }
  const refreshRuns = () => void qc.invalidateQueries({ queryKey: ['research-runs'] })
  const renameRun = (run: ResearchRun) => {
    void renameResearch(run.id, researchLabel(run)).then((ok) => {
      if (ok) refreshRuns()
    })
  }
  const removeRun = (run: ResearchRun) => {
    void removeResearch(run).then((ok) => {
      if (!ok) return
      refreshRuns()
      // The run's discussion goes WITH the run (the server deletes both), so
      // the rail is stale too — a thread left on it would open onto nothing.
      refreshSessions()
      // Deselect only if the removed run IS the open session. A right-click
      // Remove on some other row in the rail must not blank the stage the
      // person is reading.
      if (run.conversationId && run.conversationId === selectedSessionId) setSelectedSessionId(null)
      else if (legacyRunId === run.id) void navigate('/work')
    })
  }

  const selectConversation = (c: Conversation) => {
    pickAgent(c.agentModel)
    setSelectedSessionId(c.id)
    // Opening a session is leaving the ask: it floats over the stream, and the
    // stream is about to be a different session's.
    researchOpen = false
  }
  // "ALL AGENTS" — the rail's filter off, which is browse. It drops the session
  // selection with it, because the alignment effect below would otherwise read
  // the open session's agent straight back over the pick.
  const selectAllAgents = () => {
    pickAgent(null)
    setSelectedSessionId(null)
    researchOpen = false
  }
  const selectAgent = (agentModel: string) => {
    pickAgent(agentModel)
    setSelectedSessionId(null)
    newChatSignal += 1
    sessionTier = ''
  }
  const newSession = () => {
    if (selectedAgent) selectAgent(selectedAgent)
  }
  const onCreated = (id: string) => {
    setSelectedSessionId(id)
    void qc.invalidateQueries({ queryKey: ['conversations'] })
  }

  // ── STARTING FROM A CARD ───────────────────────────────────────────────────
  //
  // A starter card's seed IS the first message, and the first message is what
  // CREATES a session — there is no empty conversation to type into, because
  // /api/chat writes the row. So this takes the same send path the composer's
  // first submit takes (same route, same kind, the header's own tier pick)
  // and hands the new id to the stage, where ChatView loads
  // the thread and its live-resume poller animates the reply the server is
  // already writing (see its `resuming` derivation: the last row being the
  // user's is exactly this case).
  //
  // NOT A PREFILL, and that is a property of the code rather than a
  // preference: ChatView takes no inbound text, and the composer's draft store
  // is Comms' (ChatView passes no `draftKey`), so "put the words in the box
  // and let them press Enter" is not something this surface can do without
  // changing the stream's own components.
  //
  // The reader's own stream is abandoned the moment the ids arrive. That is
  // safe and deliberate: an abort here is LOCAL (lib/chat's note on
  // `stopChat`) — the server keeps writing the turn, which is what the stage
  // is about to show.
  let starting = $state(false)
  const startFromSeed = async (seed: string, kind: 'work' | 'plan') => {
    const model = selectedAgent
    if (!model || starting) return
    starting = true
    const ctrl = new AbortController()
    let landed: string | null = null
    try {
      const stream = streamChat(
        { model, content: seed, kind, tier: sessionTier || undefined },
        (m) => {
          landed = m.conversationId
          onCreated(m.conversationId)
          ctrl.abort()
        },
        ctrl.signal,
      )
      // ONE PULL is what sends the request: `onMeta` above fires inside it,
      // before the first event, which is the whole reason this reader exists.
      await stream.next()
    } catch (e) {
      // The abort we asked for is not a failure. A refused send is, and the
      // card is the only place the person could hear about it.
      if (!landed) toastError('Could not start that session', e)
    } finally {
      starting = false
      ctrl.abort()
    }
  }
  const onStartCard = (card: StarterCard) => {
    // Research is scoped by a question the composer asks for, not by a
    // message, so its card opens the ask over the stage like the header's own
    // toggle does.
    if (!sendsSeed(card)) {
      researchOpen = true
      return
    }
    // The card's own kind: a plan card makes a plan session (its living
    // document seeds from the template), everything else a work session. The
    // server resolves the kind off the row from the second turn on, so this is
    // the one place it is decided.
    void startFromSeed(card.seed, card.kind === 'plan' ? 'plan' : 'work')
  }

  // A deep link may reference a session whose AGENT isn't the sticky pick —
  // align the agent once the list resolves (the URL itself stays put).
  $effect(() => {
    if (!selectedSessionId) return
    const target = [...conversations, ...archived].find((c) => c.id === selectedSessionId)
    if (target && target.agentModel !== sticky.selected) pickAgent(target.agentModel)
  })

  const current = $derived(agents.find((a) => a.id === selectedAgent))
  // WHO THE STREAM SAYS IS ANSWERING. The fleet's own label when the agent is
  // still in the fleet, and otherwise what the run itself recorded — a run
  // outlives the agent that ran it, and "the researcher" is better than a blank
  // where a name goes. The fallback chain RunView had, for the same reason.
  const agentLabel = $derived(
    current?.label ?? selectedRun?.agentModel ?? legacyRun?.agentModel ?? 'the researcher',
  )

  // The tier chip's shape. This view lifts the pick out of its (minimal)
  // composer, so it spells out what the chip renders: '' is the agent's main
  // model and the chip's bottom rung — 'main' on the chip, 'main model' in the
  // row it stands on.
  const tierNames = $derived(current?.tiers ?? [])
  const tierOptions = $derived([
    { value: '', label: 'main model' },
    ...tierNames.map((name) => ({ value: name, label: name })),
  ])
  const tierMeter = $derived({
    total: tierOptions.length,
    lit: Math.max(0, tierOptions.findIndex((o) => o.value === sessionTier)) + 1,
  })

  const selected = $derived(
    conversations.find((c) => c.id === selectedSessionId) ??
      archived.find((c) => c.id === selectedSessionId) ??
      null,
  )
  const selectedIsArchived = $derived.by(() => {
    const c = selected
    return c != null && !conversations.some((row) => row.id === c.id)
  })
  // WHAT THE STREAM IS TALKING TO. The server resolves an existing
  // conversation's kind for itself (chat.rs reads it off the row), so this only
  // decides what a NEW one becomes — a work session — and keeps the client's
  // own reading of a research discussion honest.
  const sessionKind = $derived(selectedRun ? ('research' as const) : ('work' as const))
  const menu = useContextMenu()
  const quiet =
    'flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg'
  const refreshSessions = () => void qc.invalidateQueries({ queryKey: ['conversations'] })
  const renameSession = (c: Conversation) => {
    void renameConversation(c.id, c.title, 'session').then((ok) => {
      if (ok) refreshSessions()
    })
  }
  const archiveSession = (c: Conversation) => {
    void archiveConversation(c.id).then((ok) => {
      if (!ok) return
      refreshSessions()
      if (c.id === selectedSessionId) setSelectedSessionId(null)
    })
  }
  const restoreSession = (c: Conversation) => {
    void restoreConversation(c.id).then((ok) => {
      if (ok) refreshSessions()
    })
  }
  const deleteSession = (c: Conversation) => {
    void deleteConversation(c.id, c.title, 'session').then((ok) => {
      if (!ok) return
      refreshSessions()
      if (c.id === selectedSessionId) setSelectedSessionId(null)
    })
  }
  const openSessionMenu = (
    e: MouseEvent,
    c: Conversation,
    archivedRow: boolean,
    layout: ContextMenuEntry[] = [],
  ) => {
    // The rail's own pin/file entries ride along so the right-click menu
    // mirrors what the row's hover control offers (UI-CONVENTIONS: a context
    // menu is a shortcut to actions the surface already has).
    menu.openMenu(e, [
      ...planRowMenu(c, {
        path: `/work/${c.id}`,
        open: () => selectConversation(c),
        archived: archivedRow,
        onRename: () => renameSession(c),
        onArchive: () => archiveSession(c),
        onRestore: () => restoreSession(c),
        onDelete: () => deleteSession(c),
      }),
      ...(layout.length ? (['sep' as const, ...layout] as ContextMenuEntry[]) : []),
    ])
  }
  // A RESEARCH-ORIGIN ROW IS A RUN, and its menu acts on the run: renaming the
  // thread would leave the run's own name — the one every other research
  // surface reads — untouched, and removing the thread would leave the run
  // behind it. No Archive either: a run is ephemeral, and Remove is its stop.
  const openRowMenu = (
    e: MouseEvent,
    c: Conversation,
    archivedRow: boolean,
    layout: ContextMenuEntry[] = [],
  ) => {
    const run = runBySession.get(c.id)
    if (!run) {
      openSessionMenu(e, c, archivedRow, layout)
      return
    }
    menu.openMenu(
      e,
      researchRowMenu(run.id, () => selectConversation(c), canManageRun(run), {
        rename: () => renameRun(run),
        remove: () => removeRun(run),
      }),
    )
  }
  // THE HEADER'S ⋯ MENU: the open session's own row menu, minus "Open" (you
  // are standing in it). A research session's is the run's — rename and remove
  // act on the run, the same split the rail makes.
  const sessionMenuItems = (): ContextMenuEntry[] => {
    const notOpen = (e: ContextMenuEntry) => e === 'sep' || e.label !== 'Open'
    if (selectedRun) {
      const run = selectedRun
      return researchRowMenu(run.id, () => {}, canManageRun(run), {
        rename: () => renameRun(run),
        remove: () => removeRun(run),
      }).filter(notOpen)
    }
    const c = selected
    if (!c) return []
    return planRowMenu(c, {
      path: `/work/${c.id}`,
      open: () => {},
      archived: selectedIsArchived,
      onRename: () => renameSession(c),
      onArchive: () => archiveSession(c),
      onRestore: () => restoreSession(c),
      onDelete: () => deleteSession(c),
    }).filter(notOpen)
  }
  // WHAT THE RAIL CANNOT SAY FOR ITSELF: the merged list carries no kind on the
  // wire, so a research-origin row wears the run's own state — the dot, pulsing
  // only while it runs — and its depth. Every other row stays plain.
  const rowMeta = (c: Conversation) => {
    const run = runBySession.get(c.id)
    if (!run) return null
    return {
      status: STATUS_DOT[run.status],
      pulse: run.status === 'running',
      chip: MODE_META[run.mode].label,
    }
  }

  // The selected session names the view. Effect-claimed like Boards — the list
  // query lands after mount, and the pathname key keeps the fallback honest on
  // deep links. An unsaved "New session" claims nothing: it isn't a place yet.
  //
  // The claim is INERT right now: the dock era dropped the title row, and the
  // strip that would have read it is deleted, so nothing reads `viewTitleClaim`
  // today (see lib/view-title.svelte.ts). It stays because the sibling views
  // all made it, and the day a surface wants view titles again this one should
  // not be the view that forgot.
  $effect(() => {
    const run = standaloneRunId ? legacyRun : null
    const name = run ? researchLabel(run) : selected ? selected.title || 'Untitled session' : null
    if (!name) return
    claimViewTitle(name, { trail: [name] })
  })
</script>

{#snippet hireAction()}
  <Button size="sm" onclick={() => void navigate('/agents')}>Hire an agent</Button>
{/snippet}

{#snippet headerActions()}
  <div class="flex items-center gap-2">
    <!-- Below `sm` the row has room for the session's actions or its faces, not
         both; the actions win, because sharing is not what a phone is for. -->
    <span class="hidden sm:contents">
      {#if selectedRun}
        <!-- Sharing a research session shares the RUN: its members are the
             people who can read the report being discussed, which is a
             different plane from `conversation_members` and the only one that
             grants it. -->
        <ResearchMembers runId={selectedRun.id} />
      {:else if selectedSessionId}
        <!-- Sharing and presence, the Plan idiom's own controls: a session is a
             conversation with members, so it gets the same header row rather
             than a second one built for it. -->
        <ConversationMembers conversationId={selectedSessionId} />
      {/if}
    </span>
    {#if tierNames.length > 0}
      <!-- The harness sits beside the view's other model-level controls, not in
           the composer. -->
      <ComposerPicker
        icon="✳"
        chipVariant="primary"
        value={sessionTier}
        label={sessionTier || 'main'}
        options={tierOptions}
        meter={tierMeter}
        searchPlaceholder="Search tiers"
        menuClass="min-w-44"
        title="Model tier for this session"
        menuLabel="Model tier"
        onChange={(t) => (sessionTier = t)}
      />
    {/if}
    {#if selectedSessionId}
      <!-- WHICH COLUMN THE NARROW SCREEN IS SHOWING. Only below `lg` — above
           it both columns are up and there is nothing to choose. The second
           cell says what the pane actually holds: a run's report, a plan, or
           the session's files. -->
      <div class="lg:hidden">
        <Segmented
          size="xs"
          value={narrowView}
          onChange={(v) => (narrowView = v)}
          options={[
            { id: 'stream', label: 'Chat', title: 'The conversation' },
            {
              id: 'document',
              label: selectedRun ? 'Report' : paneKind === 'plan' ? 'Plan' : 'Files',
              title: selectedRun
                ? 'The run’s cited report'
                : paneKind === 'plan'
                  ? 'The plan this session is writing'
                  : 'The files this session is working on',
            },
          ]}
        />
      </div>
    {/if}
    <!-- THE SESSION-LEVEL ACTIONS, one quiet register: icon + mono label, the
         labels giving way below `xl` (each keeps its title and accessible
         name). Nothing here takes the accent, with one exception: a PLAN
         exists to become tickets, so there Draft tickets stays the gold
         primary. In a work session it is one action among several. -->
    <span class="flex items-center">
      {@render researchToggle()}
      <!-- THE SESSION AS A DOCUMENT, beside the clock: both are things you do
           to the thread as a whole rather than to a turn in it. Offered for a
           research thread too — the discussion is a transcript like any other,
           and the report has its own pane. -->
      <ExportButton conversationId={selectedSessionId} {agentLabel} />
      <!-- PUTTING THIS SESSION'S AGENT ON A CLOCK. The control belongs to the
           thread, so it takes the conversation id and renders nothing without
           one. Who may do it is the agent's own question — only its managers —
           and the answer is not on this surface's roster read, so the button
           is offered and the route's 403 arrives as a toast. A research thread
           is a run's discussion, not a standing session: a cron posting into it
           would be noise beside the report. -->
      <ScheduleButton conversationId={selectedRun ? null : selectedSessionId} />
      {#if selectedSessionId && !selectedRun && paneKind !== 'plan'}
        <!-- Drafting tickets reads a plan or a work session (`plans_id_draft`
             admits chat, plan and work kinds); a research thread is neither,
             so offering it there would be offering a 400. -->
        <DraftTicketsButton conversationId={selectedSessionId} variant="quiet" />
      {/if}
    </span>
    {#if selectedSessionId && paneKind === 'plan'}
      <DraftTicketsButton conversationId={selectedSessionId} />
    {/if}
    {#if sessionMenuItems().length > 0}
      <!-- Rename, archive, delete: the row menu the rail already offers for
           this session (same functions, same confirms, same owner guard),
           folded behind one control instead of three links across the header. -->
      <DropdownMenu class="shrink-0" items={sessionMenuItems}>
        {#snippet trigger(_open: boolean)}
          <IconButton size="sm" title="Session actions">
            <Ellipsis size={14} />
          </IconButton>
        {/snippet}
      </DropdownMenu>
    {/if}
  </div>
{/snippet}

{#snippet researchToggle()}
  <!-- ASKING FOR RESEARCH IS A SESSION-LEVEL ACT, not a view of its own: the
       ask opens over the stage, and the run it starts opens as a session like
       any other. -->
  <Button
    variant="ghost"
    size="xs"
    class="gap-1"
    title={researchOpen ? 'Back to the session' : 'Ask for a cited report, run by this agent'}
    aria-label={researchOpen ? 'Close research' : 'Research'}
    onclick={() => (researchOpen = !researchOpen)}
  >
    {#if researchOpen}<X size={13} /> <span class="hidden xl:inline">Close</span>{:else}<Telescope size={13} />
      <span class="hidden xl:inline">Research</span>{/if}
  </Button>
{/snippet}

{#snippet stageMeta()}
  <span class="flex items-center gap-2">
    {#if selectedRun}
      <!-- WHICH KIND OF SESSION THIS IS, said where the rail cannot say it: the
           merged list carries no kind on the wire, so a research-origin session
           names itself here — with the run's own dot, because a report in
           flight is the one thing you need to see without opening the pane. -->
      <StatusDot status={STATUS_DOT[selectedRun.status]} pulse={selectedRun.status === 'running'} />
      <Chip>{MODE_META[selectedRun.mode].label}</Chip>
    {/if}
    <!-- `agentLabel`, not the fleet row: a run outlives the agent that ran it,
         and the name it recorded is better than no name at all. -->
    {#if current || selectedRun}<span>with {agentLabel}</span>{/if}
  </span>
{/snippet}

{#snippet standaloneActions()}
  {#if standaloneRunId}
    {@const runId = standaloneRunId}
    <ResearchMembers {runId} />
    <!-- WHAT WILL GROW HERE, offered rather than described: this run has no
         discussion, so the way in is a button — and once the thread exists the
         run opens as a session like every other one. It stays in the header as
         well as in the column below, because below `lg` that column is the
         report and the invitation is not on screen. -->
    <button type="button" class={quiet} onclick={() => void openDiscussion(runId)} disabled={opening}>
      {opening ? 'Opening' : 'Start the discussion'}
    </button>
    {#if openError}<span class="font-sans text-xs text-danger">{openError}</span>{/if}
    {#if legacyRun && canManageRun(legacyRun)}
      {@const run = legacyRun}
      <button type="button" class={quiet} onclick={() => renameRun(run)}>Rename</button>
      <DangerLink onClick={() => removeRun(run)}>Remove</DangerLink>
    {/if}
    <!-- Asking is session-level everywhere else on this surface; a run reached
         at its own address is no exception, so the toggle stands here too. -->
    {@render researchToggle()}
  {/if}
{/snippet}

{#snippet discussionInvitation()}
  <!-- WHAT WILL GROW HERE, said before it exists — the same courtesy the plan
       document's empty pane pays. It is the copy the run view carried, and it
       is the honest half of the offer: the button says what happens, this says
       what the thread is FOR. -->
  <div class="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
    <p class="max-w-xs font-sans text-sm text-muted">
      Ask {agentLabel} about this report, or bring a teammate in. It answers from what it found, and can go and look
      again when the answer is not in there.
    </p>
    {#if standaloneRunId}
      {@const runId = standaloneRunId}
      <Button size="sm" onclick={() => void openDiscussion(runId)} disabled={opening}>
        {opening ? 'Opening' : 'Start the discussion'}
      </Button>
    {/if}
    {#if openError}<p class="max-w-xs font-sans text-xs text-danger">{openError}</p>{/if}
  </div>
{/snippet}

{#snippet researchAsk()}
  <!-- THE ASK, OVER THE STAGE RATHER THAN INSTEAD OF IT — the heading and the
       room to write a real question it always had, floated on the scrim the one
       modal uses, with the stream still mounted underneath. A click on the
       scrim closes it; so does the header's own Close. -->
  <div transition:fade={{ duration: 150 }} class="absolute inset-0 z-20 flex flex-col">
    <div
      class="absolute inset-0 bg-surface/80 backdrop-blur-[2px]"
      onclick={() => (researchOpen = false)}
      aria-hidden="true"
    ></div>
    <div class="relative min-h-0 flex-1 overflow-y-auto px-6 py-10">
      <div class="mx-auto flex min-h-full w-full max-w-[var(--converse-width)] flex-col justify-center">
        <div class="mb-4 text-center">
          <div class="text-lg font-medium text-fg">Research</div>
          <p class="mx-auto mt-1 max-w-md font-sans text-xs leading-relaxed text-muted">
            Recon answers fast; Brief maps a topic; Expedition goes deep. Reports are cited,
            org-visible documents your agents can retrieve later.
          </p>
        </div>
        <!-- THE RUN DISCUSSES WHERE IT WAS ASKED FOR. Opened over a selected
             session, the ask points the run at that thread: its scope
             question, its ack and its report-ready turn land in the
             conversation the person is already reading, rather than opening a
             second row beside it. From the empty stage there is no thread to
             name, and the run makes its own — which is also the standalone
             report's address, where `selectedSessionId` is null. -->
        <ResearchComposer
          agentModel={selectedAgent}
          onStarted={onResearchStarted}
          conversationId={selectedSessionId}
        />
      </div>
    </div>
  </div>
{/snippet}

{#snippet orphanGroup()}
  {#if orphanRuns.length > 0}
    <!-- RUNS WITH NO THREAD, WHERE THERE IS ROOM TO LIST THEM. The rail lists
         conversations and these have none, so they would be unreachable but for
         a group of their own: every row opens the report at its own address,
         which is also where the way into a discussion is offered. -->
    <div class="mx-auto w-full max-w-[var(--converse-width)] px-4 pb-6">
      <div class="mb-1 px-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Research (no thread)</div>
      <div class="space-y-0.5">
        {#each orphanRuns as r (r.id)}
          <button
            type="button"
            onclick={() => void navigate('/research/:runId', { params: { runId: r.id } })}
            oncontextmenu={(e) =>
              menu.openMenu(
                e,
                researchRowMenu(
                  r.id,
                  () => void navigate('/research/:runId', { params: { runId: r.id } }),
                  canManageRun(r),
                  { rename: () => renameRun(r), remove: () => removeRun(r) },
                ),
              )}
            class="block w-full rounded-md px-2 py-1.5 text-left transition-colors dither-fill"
          >
            <div class="flex items-center gap-2">
              <StatusDot status={STATUS_DOT[r.status]} pulse={r.status === 'running'} class="h-1.5 w-1.5" />
              <span class="min-w-0 flex-1 truncate text-sm text-fg">{researchLabel(r)}</span>
            </div>
            <!-- §10 session-row meta: 10px mono chrome voice. -->
            <div class="mt-0.5 flex items-center gap-2 pl-3.5 font-mono text-[10px] tracking-[0.05em] text-muted">
              <Chip>{MODE_META[r.mode].label}</Chip>
              <span class="truncate">{r.requestedBy}</span>
              <span class="ml-auto shrink-0">{relativeTime(r.createdAt)}</span>
            </div>
          </button>
        {/each}
      </div>
    </div>
  {/if}
{/snippet}

{#snippet stripTrailing()}
  {#if showAllFiles}
    <!-- Every file the session has, the closed ones too, so a closed file is
         one click from coming back. -->
    <DropdownMenu items={allFilesItems}>
      {#snippet trigger(_open: boolean)}
        <Button variant="ghost" size="xs" class="gap-1" title="Every file in this session">
          <Files size={12} />
          All files ({sessionFiles.length})
        </Button>
      {/snippet}
    </DropdownMenu>
  {/if}
  <DropdownMenu items={openItems}>
    {#snippet trigger(_open: boolean)}
      <IconButton size="sm" title="Open a file in this session" disabled={addingFile}>
        <Plus size={14} />
      </IconButton>
    {/snippet}
  </DropdownMenu>
{/snippet}

{#snippet filesEmptyActions()}
  <div class="flex flex-wrap items-center justify-center gap-2">
    <Button size="sm" variant="outline" disabled={addingFile} onclick={() => void newDocument()}>New document</Button>
    <Button size="sm" variant="ghost" disabled={addingFile} onclick={() => void openGoogleFile()}>
      Open a Google file
    </Button>
  </div>
{/snippet}

{#snippet stageHeader()}
  {#if standaloneRunId && legacyRun}
    <!-- No title: the strip carries the run's name (claimed above); the row
         keeps the run's meta and the actions. -->
    <StageHeader meta={`${MODE_META[legacyRun.mode].label} · ${legacyRun.agentModel}`} actions={standaloneActions} />
  {:else if selectedAgent && (current || selectedRun)}
    <!-- No title either: a selected session's name lives in the strip, and an
         unsaved one has no name worth a header. The row keeps the agent, the
         session's state and the session-level controls. -->
    <StageHeader meta={stageMeta} actions={headerActions} />
  {/if}
{/snippet}

<RailSurface>
  <ConversationSidebar
    {agents}
    {conversations}
    {archived}
    {selectedAgent}
    selectedConversationId={selectedSessionId}
    {agentsLoading}
    {conversationsLoading}
    {agentsFailure}
    {conversationsFailure}
    {archivedFailure}
    onSelectAgent={selectAgent}
    onSelectAll={selectAllAgents}
    onSelectConversation={selectConversation}
    onNewChat={newSession}
    onRowMenu={openSessionMenu}
    {rowMeta}
    rowMenu={openRowMenu}
    noun="session"
    newPerm="work.sessions"
    newTitle="New session: work with the agent on your files"
    emptyHint="A session is you and the agent working together: the conversation here, the files it writes beside it. Reads are free; anything it writes to Google waits for your approval."
    collapseKey="work"
  />

  <Stage header={stageHeader}>
    {#if legacyRunId}
      <!-- A RUN'S OLD ADDRESS, from the first paint: the report pane renders
           the run's own loading shape while the run resolves, and either the
           hop to its session lands (and this branch goes with it) or the run
           has no discussion to hop to and its report IS the surface. Falling
           through to the session branch meanwhile would greet a mailed link
           with an empty composer for a beat. `relative`, because the research
           ask floats over this body and an overlay needs something to be
           positioned against. -->
      <div class="relative flex h-full min-h-0 flex-col">
        {#if researchOpen}{@render researchAsk()}{/if}
        <NoModelBump class="m-4 shrink-0" />
        <div class="flex min-h-0 flex-1">
          {#if standaloneRunId}
            <!-- The same two columns a research SESSION stands in — what there
                 is to say on the left, the report on the right — with the
                 invitation where the stream would be. Hidden below `lg` for the
                 reason the session's own stream is: one column at that width,
                 and the report is the one worth having. -->
            <div class="hidden min-w-0 basis-[46%] flex-col border-r border-line-subtle lg:flex">
              <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
                <MessageSquare size={13} class="text-muted" aria-hidden="true" />
                <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Discussion</span>
              </div>
              {@render discussionInvitation()}
            </div>
          {/if}
          <ReportPane runId={legacyRunId} showSources />
        </div>
      </div>
    {:else if selectedAgent && (current || selectedRun)}
      <!-- `relative`: the research ask is an overlay over this stage body. It
           used to be a BRANCH here, which unmounted ChatView to render — and
           unmounting the stream mid-reply aborts the reply it was waiting on.
           Over the top, the stream keeps streaming behind it and closing the
           ask puts you back where you were. -->
      <div class="relative flex h-full min-h-0">
        {#if researchOpen}{@render researchAsk()}{/if}
        <!-- Below `lg` the stage shows ONE of these two columns and the header's
             toggle says which; at `lg` and up both are open and the toggle is
             gone. -->
        <div class={cn('min-w-0 flex-1 flex-col', narrowView === 'document' ? 'hidden lg:flex' : 'flex')}>
          <!-- Above the composer, not instead of it: a session with no model
               behind it still opens, and the fix is offered here. -->
          <NoModelBump class="m-4 shrink-0" />
          <div class="min-h-0 flex-1">
            {#key selectedAgent}
              <ChatView
                agentModel={selectedAgent}
                {agentLabel}
                conversationId={selectedSessionId}
                {newChatSignal}
                {onCreated}
                kind={sessionKind}
                minimal
                tier={sessionTier}
                syncSignal={chatSyncSignal}
                {onToolDone}
                {mentionables}
                {onTurnComplete}
                focus={chatFocus}
              />
            {/key}
          </div>
        </div>
        <!-- THE WORKSPACE. Two-pane from the FIRST keystroke, so the surface
             never re-layouts mid-thought. `lg:block` rather than `lg:flex`: the pane
             inside is the flex container, and a flex child with no basis of its
             own would shrink-wrap its content instead of filling the column.
             Below `lg` it takes the whole stage when the toggle points at it,
             and the stream column is the one standing down. -->
        <div
          class={cn(
            'min-w-0 basis-full border-l border-line-subtle lg:block lg:basis-[44%]',
            narrowView === 'document' ? 'block' : 'hidden',
          )}
        >
          {#if selectedSessionId}
            {@const sessionId = selectedSessionId}
            <!-- The strip's trailing controls (the file list, the way to open
                 another) stand once the session has anything on it; before
                 that, the empty state below carries the same two ways in. -->
            <WorkspacePane
              {sessionId}
              onClose={closeWorkspaceTab}
              trailing={paneKind !== null && (strip.tabs.length > 0 || sessionFiles.length > 0) ? stripTrailing : undefined}
            >
              {#snippet children(tab)}
                <!-- The row the panes expect: they take their own `flex-1` and
                     scroll themselves, which needs a parent with a height. -->
                <div class="flex h-full min-h-0">
                  {#if tab?.kind === 'report' && reportRunId}
                    <!-- The run's report, with its source registry under it:
                         the body already ends in a `## Sources` section, but
                         the rows are worth skimming beside the prose. -->
                    <ReportPane runId={reportRunId} showSources />
                  {:else if tab?.kind === 'drafts'}
                    <!-- THE DRAFTS, WHERE THE SESSION CAN SEE THEM. The review
                         itself is the modal the button opens — it is a walk
                         through a batch and wants the width — so what belongs
                         in the pane is the job's state and the way back into
                         it. -->
                    <div class="min-w-0 flex-1 overflow-y-auto p-6">
                      <div class="mx-auto w-full max-w-[var(--converse-width)]">
                        <Panel class="space-y-3">
                          <div class="font-mono text-[11px] uppercase tracking-[0.08em] text-ink-dim">Tickets</div>
                          <p class="font-sans text-sm text-muted">{draftSentence}</p>
                          <div class="flex"><DraftTicketsButton conversationId={sessionId} /></div>
                        </Panel>
                      </div>
                    </div>
                  {:else if tab && (tab.kind === 'artifact' || tab.kind === 'google') && fileByKey.has(tab.id)}
                    {@const file = fileByKey.get(tab.id)!}
                    <!-- ONE FILE: one header row, the file under it. Keyed on
                         the file, so moving between tabs resets the comments
                         and fullscreen a file had open; a refetch of the list
                         hands the same key a fresh object and remounts nothing. -->
                    {#key file.key}
                      <FilePane
                        {file}
                        onClose={() => closeWorkspaceTab(tab)}
                        onDeleted={() => {
                          closeTab(sessionId, tab.id)
                          void refreshFiles(sessionId)
                        }}
                      />
                    {/key}
                  {:else if tab?.kind === 'doc' || paneKind === 'plan'}
                    <!-- A PLAN'S OWN DOCUMENT — the one session kind that keeps a
                         living document, its sync from chat, and drafting as
                         its point. -->
                    <LivingDocPane
                      conversationId={sessionId}
                      syncSignal={turnSignal}
                      onDocId={(id) => (livingDocId = id)}
                    />
                  {:else if filesFailed}
                    <div class="grid min-w-0 flex-1 place-items-center p-6">
                      <QueryError
                        error={filesQuery.error}
                        title="Could not load this session’s files"
                        onRetry={() => void filesQuery.refetch()}
                      />
                    </div>
                  {:else if filesPending || paneKind === null || runsQuery.isLoading}
                    <!-- Until the list lands we do not know what stands here;
                         the document shape is the honest stand-in for a pane
                         that will most likely hold one. -->
                    <PlanDocSkeleton />
                  {:else}
                    <!-- NOTHING IS OPEN, said where the file would be, with the
                         two ways to change that. -->
                    <div class="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
                      <EmptyState
                        class="min-h-0 flex-1"
                        icon="◫"
                        title={sessionFiles.length > 0 ? 'Nothing open' : 'Nothing open yet'}
                        hint={sessionFiles.length > 0
                          ? 'Every file in this session is closed. Reopen one from All files, or open another.'
                          : `Ask ${agentLabel} for a document, sheet or page and it opens here, or open one yourself.`}
                        action={filesEmptyActions}
                      />
                    </div>
                  {/if}
                </div>
              {/snippet}
            </WorkspacePane>
          {:else}
            <!-- No session yet, so nothing is open — but the column still wears
                 the pane's header row, so the pane that lands here is the same
                 pane that was standing here. `full` on the empty state carries
                 the dithered vignette, which keeps a 44% column that owns half
                 the stage from reading as a dead void. -->
            <div class="flex h-full min-h-0 flex-col">
              <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
                <Files size={13} class="text-muted" aria-hidden="true" />
                <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Files</span>
              </div>
              <div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
                <!-- The vignette keeps the space it is given, and the group
                     below keeps its own: a run with no thread is reachable
                     without scrolling for it. -->
                <div class="min-h-48 flex-1">
                  <EmptyState
                    icon="◫"
                    title="Nothing open yet"
                    hint={`Ask ${agentLabel} for a document, sheet or page and it opens here.`}
                  />
                </div>
                <!-- THE WAYS IN, under the vignette and above the orphan runs:
                     the empty state says what is absent, and these say what to
                     do about it — here, where the agent is chosen and a card
                     can actually start a session with it. `shrink-0` like the
                     group below, so neither block squeezes the other. -->
                <div class="shrink-0">
                  <StarterCards onStart={onStartCard} disabledReason={starting ? 'Starting the session' : null} />
                </div>
                <div class="shrink-0">{@render orphanGroup()}</div>
              </div>
            </div>
          {/if}
        </div>
      </div>
    {:else if agentsLoading}
      <!-- The stage frame renders immediately while the fleet loads: the same
           two-pane layout (session column + 44% workspace column) it stands in
           for, so nothing pops or re-layouts when agents land. -->
      <div aria-hidden="true" class="flex h-full min-h-0">
        <div class="min-w-0 flex-1 overflow-hidden p-6">
          <div class="mx-auto flex w-full max-w-[var(--converse-width)] flex-col gap-4">
            <!-- Chat flattens onto the panel (spec §10) — the stand-in blocks
                 wear the same radius-8 panels the messages will. -->
            <Skeleton class="h-14 w-3/5 self-end rounded-lg" />
            <Skeleton class="h-24 w-4/5 rounded-lg" />
            <Skeleton class="h-12 w-1/2 self-end rounded-lg" />
            <Skeleton class="h-20 w-3/4 rounded-lg" />
          </div>
        </div>
        <div class="hidden min-w-0 basis-[44%] flex-col border-l border-line-subtle lg:flex">
          <!-- The document's own stand-in, not a grey block: the pane that
               lands here has a toolbar row and prose, and PlanDocSkeleton is
               that shape. -->
          <PlanDocSkeleton />
        </div>
      </div>
    {:else if agentsFailure}
      <!-- "No agents available." is a statement about the FLEET. When the fleet
           read itself failed, the only honest thing to report is that. -->
      <div class="grid h-full place-items-center">
        <QueryError error={agentsFailure.error} title="Could not load your agents" onRetry={agentsFailure.retry} />
      </div>
    {:else if agents.length === 0}
      <!-- A statement about the FLEET, so it says what to do about it — bare
           "No agents available." named the absence and offered no way out of
           it. The action only appears for someone who can act on it: /agents is
           a Manage view, and offering a button that bounces is worse than
           offering none. -->
      <EmptyState
        icon="◇"
        title="No agents yet"
        hint="A session is a conversation with one of your agents. Hire one and it shows up here."
        action={mayManageAgents.current ? hireAction : undefined}
      />
    {:else}
      <!-- "ALL AGENTS" WITH NOTHING OPEN: the rail is showing everybody's
           sessions and the stage is waiting to be pointed at one. The copy is
           the research page's, because the state is the same state — a filter
           turned off, not an absence of anything. -->
      <!-- `relative`, and the scroller is a child rather than this element:
           the research ask floats over this stage too (a starter card opens
           it), and an overlay inside the scroll box would scroll with the
           content it is covering. -->
      <div class="relative flex h-full min-h-0 flex-col">
        {#if researchOpen}{@render researchAsk()}{/if}
        <div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
          <div class="min-h-48 flex-1">
            <EmptyState icon="◎" title="Nothing open" hint="Pick an agent above, or start one. It lands here." />
          </div>
          <div class="shrink-0">{@render orphanGroup()}</div>
        </div>
      </div>
    {/if}
  </Stage>

  <ContextMenu {menu} />
</RailSurface>

<!-- ⌘K: jump to a session. The palette owns its shortcut and its open state. -->
<SessionPalette {conversations} onSelect={(id) => setSelectedSessionId(id)} />
