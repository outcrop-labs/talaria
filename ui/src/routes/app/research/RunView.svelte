<script lang="ts">
  import { ExternalLink, MessageSquare } from '@lucide/svelte'
  import { p } from '@/router'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Button from '@/components/ui/Button.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import ChatView from '@/components/chat/ChatView.svelte'
  import { useArtifact } from '@/lib/artifacts'
  import { openResearchConversation, useResearchMembers, useResearchRun } from '@/lib/research'
  import { useAgents } from '@/lib/agents'
  import { userMentionInsert } from '@/components/chat/mentions.svelte'
  import ReportSkeleton from './ReportSkeleton.svelte'
  import ResearchMembers from './ResearchMembers.svelte'

  // A RUN IS A CONVERSATION WITH A REPORT BESIDE IT — the shape the Plan surface
  // already proved: several people, one agent, one document that grows.
  //
  // WHAT IT REPLACED. This view was the report and nothing else, so the only
  // thing anyone could do with a finished run was read it again, and the
  // colleagues it was shared with had nowhere to say "dig into the second point"
  // or "that source is a vendor blog". A view whose whole content is a document
  // does not need to be a view; it needs to be a document. It earns the view by
  // being where the work continues.
  //
  // TALK LEFT, REPORT RIGHT, matching Plan, so anyone who has used that surface
  // already knows this one. Below `lg` the report IS the page and the discussion
  // is not rendered at all: a phone is for reading the answer, not for holding a
  // working session, and half a chat pane on a narrow screen is worse than none.
  let { runId }: { runId: string } = $props()

  const runQuery = useResearchRun(() => runId)
  const run = $derived(runQuery.data?.run)
  const artifactQuery = useArtifact(() => run?.artifactId ?? null)
  const artifact = $derived(artifactQuery.data)
  const artifactLoading = $derived(artifactQuery.isLoading)

  const fleetQuery = useAgents()
  const agents = $derived(fleetQuery.data?.agents ?? [])
  const agentLabel = $derived(agents.find((a) => a.id === run?.agentModel)?.label ?? run?.agentModel ?? 'the researcher')

  // THE PEOPLE IN THE ROOM are the run's members — the same list that decides
  // who may read the report — so nobody is offered a mention for a teammate who
  // could not open the thing being discussed.
  const membersQuery = useResearchMembers(() => runId)
  const mentionables = $derived(
    (membersQuery.data?.members ?? [])
      .map((m) => ({ insert: userMentionInsert({ name: m.name, email: m.email }), label: m.name ?? m.email ?? m.userId, sub: m.email ?? undefined }))
      // A member with neither a name nor an email cannot be mentioned in a way
      // the notifier can resolve, so offering them would be an @ that goes
      // nowhere. Same filter the Plan surface applies.
      .filter((m) => m.insert),
  )

  // Created on the first message rather than with the run: most runs are read
  // once and never discussed, and a conversation per run would turn the chat
  // list into a list of things nobody said anything about.
  let opening = $state(false)
  let openError = $state<string | null>(null)
  let opened = $state<string | null>(null)
  const conversationId = $derived(opened ?? run?.conversationId ?? null)

  const openThread = async () => {
    opening = true
    openError = null
    const out = await openResearchConversation(runId)
    opening = false
    if (out.error) openError = out.error
    else opened = out.conversationId ?? null
  }
</script>

{#if !run}
  <!-- The shape of the run view to come: meta row, status panel, report panel. -->
  <div
    aria-hidden="true"
    class="mx-auto w-full max-w-[var(--converse-width)] space-y-4 p-8"
    style:padding-bottom="calc(var(--research-composer, 0px) + 1rem)"
  >
    <Skeleton class="h-6 w-64" />
    <ReportSkeleton />
  </div>
{:else}
  <div class="flex min-h-0 flex-1">
    <!-- ── The discussion ──────────────────────────────────────────────────── -->
    <div class="hidden min-w-0 basis-[46%] flex-col border-r border-line-subtle lg:flex">
      <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
        <MessageSquare size={13} class="text-muted" aria-hidden="true" />
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Discussion</span>
        <span class="ml-auto"></span>
        <ResearchMembers {runId} />
      </div>

      {#if conversationId}
        <!-- ChatView carries its own composer at its bottom; the research ask
             floats over this column too, so reserve its height or the two
             inputs overlap. -->
        <div class="min-h-0 flex-1" style:padding-bottom="var(--research-composer, 0px)">
          <ChatView
            agentModel={run.agentModel}
            {agentLabel}
            {conversationId}
            newChatSignal={0}
            onCreated={() => {}}
            kind="research"
            minimal
            {mentionables}
          />
        </div>
      {:else}
        <!-- WHAT WILL GROW HERE, said before it exists — the same courtesy the
             Plan surface pays its empty document pane. -->
        <div class="flex min-h-0 flex-1 flex-col items-center justify-center gap-3 p-8 text-center">
          <p class="max-w-xs font-sans text-sm text-muted">
            Ask {agentLabel} about this report, or bring a teammate in. It answers from what it found, and can go and look
            again when the answer is not in there.
          </p>
          <Button size="sm" onclick={openThread} disabled={opening}>{opening ? 'Opening' : 'Start the discussion'}</Button>
          {#if openError}<p class="max-w-xs font-sans text-xs text-danger">{openError}</p>{/if}
        </div>
      {/if}
    </div>

    <!-- ── The report ──────────────────────────────────────────────────────── -->
    <div class="min-w-0 flex-1 overflow-y-auto">
      <!-- The report scrolls UNDER the floating ask; this is what stops its
           last line parking behind it. -->
      <div
        class="mx-auto w-full max-w-[var(--converse-width)] space-y-4 p-8"
        style:padding-bottom="calc(var(--research-composer, 0px) + 1rem)"
      >
        <!-- Run meta line — chrome voice: 11px mono muted (spec §2). -->
        <div class="flex items-center gap-2 font-mono text-[11px] text-muted">
          <Avatar name={run.agentModel} class="h-6 w-6 text-[10px]" />
          <span>by {run.requestedBy}</span>
          {#if run.stats.sources !== undefined}<span>· {run.stats.sources} sources ({run.stats.cited} cited)</span>{/if}
          <span class="ml-auto"></span>
          <!-- Sharing lives in the discussion header on wide screens; below that
               breakpoint there is no discussion pane, so this is its only home. -->
          <span class="lg:hidden"><ResearchMembers {runId} /></span>
          {#if run.artifactId}
            <a href={`${p('/artifacts')}?a=${run.artifactId}`} class="flex shrink-0 items-center gap-1 text-muted transition-colors hover:text-fg">
              Open in Artifacts <ExternalLink size={12} />
            </a>
          {/if}
        </div>

        {#if run.status === 'queued' || run.status === 'running'}
          <Panel class="flex items-center gap-3">
            <WaitingMark site="research/run" size={16} class="text-accent" />
            <div class="min-w-0">
              <div class="text-sm text-fg">{run.status === 'queued' ? 'Queued' : 'Researching'}</div>
              {#if run.phase}<div class="truncate text-xs text-muted">{run.phase}</div>{/if}
            </div>
          </Panel>
        {/if}
        {#if run.status === 'error'}
          <Panel>
            <div class="text-sm text-danger">{run.error ?? 'The run failed.'}</div>
          </Panel>
        {/if}

        <!-- The report body holds its shape while the artifact fetch is in
             flight — no null-then-pop when the document lands. -->
        {#if run.artifactId && !artifact && artifactLoading}<ReportSkeleton />{/if}
        {#if artifact}
          <Panel>
            <Markdown class="prose-sm" children={artifact.body} />
          </Panel>
        {/if}

        {#if runQuery.data && runQuery.data.sources.length > 0 && run.status === 'done' && !artifact}
          <Panel>
            <div class="text-xs text-muted">Report document is loading</div>
          </Panel>
        {/if}
      </div>
    </div>
  </div>
{/if}
