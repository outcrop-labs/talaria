<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Panel from '@/components/ui/Panel.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage, postJson } from '@/lib/fetch-json'
  import { listStagger, slide } from '@/lib/motion'
  import { pushToast } from '@/lib/toast.svelte'
  import CollectionRow from './CollectionRow.svelte'
  import HealthDot from './HealthDot.svelte'
  import RerankSection from './RerankSection.svelte'
  import { useCollections, useRagAdmin } from './retrieval'

  // Admin governance of the RAG plane: service health + backfill, the brains
  // (auto + custom collections with who-can-search bindings), which KB spaces
  // feed which brain, and the reranker provider.
  const qc = useQueryClient()
  const collectionsQuery = useCollections()
  const ragQuery = useRagAdmin()
  const rag = $derived(ragQuery.data)
  const ragPending = $derived(ragQuery.isPending)
  const ragFailed = $derived(ragQuery.isError && rag === undefined)
  let name = $state('')
  let busy = $state(false)

  const create = async () => {
    if (!name.trim()) return
    busy = true
    try {
      await postJson('/api/rag/collections', { name: name.trim() })
      name = ''
      await qc.invalidateQueries({ queryKey: ['rag-collections'] })
    } catch (e) {
      pushToast({ title: 'Create failed', body: errorMessage(e), tone: 'danger' })
    } finally {
      busy = false
    }
  }
  const backfill = async (action: 'backfill' | 'reindex' = 'backfill') => {
    try {
      await postJson('/api/admin/rag', { action })
    } catch (e) {
      pushToast({ title: action === 'reindex' ? 'Rebuild failed' : 'Backfill failed', body: errorMessage(e), tone: 'danger' })
      return
    }
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }
  const rebuilding = $derived(rag?.reindex.state === 'running')
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="Retrieval"
    info="The org's RAG brains. Workspace activity and Organization knowledge are automatic. Spin up more for a domain or team, bind who can search each, and point KB spaces at them to curate what they contain."
  />

  <!-- Services + backfill. A failure here must NOT paint the health dots
       red — "we could not ask" is not "the vector store is down". -->
  {#if ragPending}<Skeleton class="mb-4 h-10 w-full rounded-md" />{/if}
  {#if ragFailed}
    <div transition:slide={{ duration: 150 }}>
      <QueryError
        class="mb-4"
        variant="compact"
        error={ragQuery.error}
        title="Could not load retrieval status"
        onRetry={() => void ragQuery.refetch()}
      />
    </div>
  {/if}
  {#if rag}
    <div class="mb-4 flex flex-wrap items-center gap-3 rounded-md border border-line p-3">
      <HealthDot ok={rag.health.qdrant} label="Vector store" />
      <HealthDot ok={rag.health.embeddings} label="Embeddings" />
      {#if rag.upgrade?.embed}
        <span class="text-xs text-muted" title="Change it via TALARIA_EMBED_MODEL (docker/dev-compose.yml), restart the embeddings container, then rebuild here.">
          model <span class="font-mono text-[11px] text-fg">{rag.upgrade.embed.modelId}</span> · <span class="font-mono text-[11px]">{rag.upgrade.embed.dim}d</span>
        </span>
      {/if}
      <span class="ml-auto font-mono text-[10px] tracking-[0.05em] text-muted">
        {#if rebuilding}
          <span class="flex items-center gap-1.5"><WaitingMark site="admin/retrieval-reindex" size={11} /> {rag.reindex.phase === 'backfilling' ? 'refilling from sources' : 'rebuilding collections'}</span>
        {:else if rag.backfill.state === 'running'}
          <span class="flex items-center gap-1.5"><WaitingMark site="admin/retrieval-backfill" size={11} /> backfilling</span>
        {:else if rag.reindex.state === 'error'}
          <span class="text-danger">rebuild failed: {rag.reindex.error}</span>
        {:else if rag.backfill.state === 'done' && rag.backfill.counts}
          {`last backfill: ${Object.entries(rag.backfill.counts).map(([k, v]) => `${v} ${k}`).join(' · ')}`}
        {:else if rag.backfill.state === 'error'}
          <span class="text-danger">backfill failed: {rag.backfill.error}</span>
        {/if}
      </span>
      <Button size="sm" variant="outline" onclick={() => void backfill()} disabled={rebuilding || rag.backfill.state === 'running' || !rag.health.qdrant || !rag.health.embeddings}>
        Backfill
      </Button>
    </div>
  {/if}

  <!-- Rebuild banner: the embedding model changed (dims no longer match) or
       a brain predates hybrid keyword search. One button repairs both. -->
  {#if rag?.upgrade?.needsReindex && !rebuilding}
    <div
      transition:slide={{ duration: 150 }}
      class={cn(
        'mb-4 flex flex-wrap items-center gap-3 rounded-md border p-3',
        rag.upgrade.dimMismatch ? 'border-danger' : 'border-warning',
      )}
    >
      <div class="min-w-0 flex-1 text-xs">
        <div class="font-semibold text-fg">
          {rag.upgrade.dimMismatch ? 'Embedding model changed; brains need a rebuild' : 'Hybrid keyword search available'}
        </div>
        <div class="mt-0.5 text-muted">
          {rag.upgrade.dimMismatch
            ? `${rag.upgrade.collections.filter((c) => c.dimMismatch).map((c) => c.name).join(', ')} no longer match the ${rag.upgrade.embed?.dim}d model, so indexing and search against them are failing.`
            : 'These brains predate keyword+semantic search; a rebuild upgrades them so exact names, env vars, and error strings rank alongside meaning.'}
          Rebuilding recreates each brain and refills it from the workspace's own records; knowledge search runs thin until the refill finishes.
        </div>
      </div>
      <Button size="sm" onclick={() => void backfill('reindex')} disabled={!rag.health.qdrant || !rag.health.embeddings}>
        Rebuild index
      </Button>
    </div>
  {/if}

  <div class="space-y-3">
    <QueryState
      query={collectionsQuery}
      errorTitle="Could not load the org's brains"
      errorVariant="compact"
    >
      {#snippet skeleton()}
        {#each [0, 1, 2] as i (i)}<SkeletonCard />{/each}
      {/snippet}
      {#snippet children(collections)}
        <div class="space-y-3" use:listStagger>
          {#each collections as c (c.id)}<CollectionRow col={c} spaces={rag?.spaces ?? []} />{/each}
        </div>
      {/snippet}
    </QueryState>
  </div>
  <!-- Creating a brain while the list is unknown risks a duplicate of one
       that is already there — hold the box until we know what exists. -->
  <div class="mt-4 flex items-center gap-2 border-t border-line-subtle pt-3">
    <Input size="sm" bind:value={name} placeholder="New brain (e.g. Sales playbook)" class="flex-1" onkeydown={(e) => e.key === 'Enter' && void create()} />
    <Button size="sm" onclick={() => void create()} disabled={busy || !name.trim() || collectionsQuery.data === undefined}>
      Create
    </Button>
  </div>

  {#if ragPending}
    <!-- Reranker placeholder: label bar + control row, same footprint. -->
    <div class="mt-5 border-t border-line-subtle pt-4">
      <Skeleton class="mb-3 h-3 w-24 rounded-full" />
      <div class="flex flex-wrap items-center gap-2">
        <Skeleton class="h-8 w-52" />
        <Skeleton class="h-8 w-64" />
      </div>
    </div>
  {/if}
  {#if rag}<RerankSection {rag} />{/if}
</Panel>
