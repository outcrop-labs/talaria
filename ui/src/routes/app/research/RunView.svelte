<script lang="ts">
  import { ExternalLink } from '@lucide/svelte'
  import { p } from '@/router'
  import Avatar from '@/components/ui/Avatar.svelte'
  import GeneratingHelix from '@/components/ui/GeneratingHelix.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { useArtifact } from '@/lib/artifacts'
  import { useResearchRun } from '@/lib/research'
  import ReportSkeleton from './ReportSkeleton.svelte'
  import ResearchMembers from './ResearchMembers.svelte'

  let { runId }: { runId: string } = $props()

  const runQuery = useResearchRun(() => runId)
  const run = $derived(runQuery.data?.run)
  const artifactQuery = useArtifact(() => run?.artifactId ?? null)
  const artifact = $derived(artifactQuery.data)
  const artifactLoading = $derived(artifactQuery.isLoading)
</script>

{#if !run}
  <!-- The shape of the run view to come: meta row, status panel, report panel. -->
  <div aria-hidden="true" class="mx-auto w-full max-w-[var(--chat-content-max-width)] space-y-4 p-8">
    <div class="flex items-center gap-2">
      <Skeleton class="h-6 w-6 shrink-0 rounded-full" />
      <Skeleton class="h-2.5 w-32 rounded-full" delay={0.12} />
      <Skeleton class="h-2.5 w-24 rounded-full" delay={0.24} />
    </div>
    <Panel class="flex items-center gap-3">
      <Skeleton class="h-4 w-4 shrink-0 rounded-full" />
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-28 rounded-full" delay={0.12} />
        <Skeleton class="h-2.5 w-48 rounded-full" delay={0.24} />
      </div>
    </Panel>
    <ReportSkeleton />
  </div>
{:else}
  <div class="mx-auto w-full max-w-[var(--chat-content-max-width)] space-y-4 p-8">
    <!-- Run meta line — chrome voice: 11px mono muted (spec §2). -->
    <div class="flex items-center gap-2 font-mono text-[11px] text-muted">
      <Avatar name={run.agentModel} class="h-6 w-6 text-[10px]" />
      <span>by {run.requestedBy}</span>
      {#if run.stats.sources !== undefined}<span>· {run.stats.sources} sources ({run.stats.cited} cited)</span>{/if}
      <span class="ml-auto"></span>
      <ResearchMembers {runId} />
      {#if run.artifactId}
        <a href={p('/artifacts')} class="flex shrink-0 items-center gap-1 text-muted transition-colors hover:text-fg">
          Open in Artifacts <ExternalLink size={12} />
        </a>
      {/if}
    </div>

    {#if run.status === 'queued' || run.status === 'running'}
      <Panel class="flex items-center gap-3">
        <GeneratingHelix />
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
{/if}
