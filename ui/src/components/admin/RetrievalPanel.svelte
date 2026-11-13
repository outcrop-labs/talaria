<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { postJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'
  import { toastError } from '@/lib/toast.svelte'
  import CollectionRow from './CollectionRow.svelte'
  import NoticeRow from './NoticeRow.svelte'
  import RerankSection from './RerankSection.svelte'
  import RowStatus from './RowStatus.svelte'
  import { indexState, useCollections, useRagAdmin, type RagCollection } from './retrieval'

  // Admin governance of the RAG plane: the collections (auto + custom, with
  // who-can-search bindings and the KB spaces that feed each), service health
  // and backfill, and the reranker provider.
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
      toastError('Create failed', e)
    } finally {
      busy = false
    }
  }
  const backfill = async (action: 'backfill' | 'reindex' = 'backfill') => {
    try {
      await postJson('/api/admin/rag', { action })
    } catch (e) {
      toastError(action === 'reindex' ? 'Rebuild failed' : 'Backfill failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['rag-admin'] })
  }
  const rebuilding = $derived(rag?.reindex.state === 'running')
  const servicesUp = $derived(!!rag?.health.qdrant && !!rag.health.embeddings)
  const canRebuild = $derived(servicesUp && !rebuilding)
  const indexOf = (id: string) => rag?.upgrade?.collections.find((c) => c.id === id)
  const isBroken = (c: RagCollection) => {
    const index = indexOf(c.id)
    return !!index && indexState(index).broken
  }
  // A collection search fails in is what an admin must see first.
  const brokenFirst = (list: RagCollection[]) => [...list.filter(isBroken), ...list.filter((c) => !isBroken(c))]
  // A rebuild that only upgrades (keyword search on older collections) has no
  // broken row to carry its button.
  const upgradeOnly = $derived(
    !!rag?.upgrade?.needsReindex && !rebuilding && !rag.upgrade.collections.some((c) => indexState(c).broken),
  )
  const counts = (c: Record<string, number>) =>
    Object.entries(c)
      .map(([k, v]) => `${v.toLocaleString()} ${k}`)
      .join(', ')
</script>

<div class="space-y-10">
  <SettingsSection
    title="Collections"
    description="What agents and people can search, and who may search each one."
    info="Workspace activity and the knowledge base are built in. Add a collection for a domain or a team, choose who can search it, and point knowledge spaces at it to decide what it holds."
  >
    <SettingsGroup>
      {#if rebuilding}
        <SettingsRow label={rag?.reindex.phase === 'backfilling' ? 'Refilling collections from your records' : 'Rebuilding collections'} hint="Search is thin until this finishes.">
          {#snippet lead()}<WaitingMark site="admin/retrieval-reindex" size={14} />{/snippet}
        </SettingsRow>
      {:else if rag?.reindex.state === 'error'}
        <NoticeRow status="danger">The rebuild failed: {rag.reindex.error}</NoticeRow>
      {/if}
      {#if upgradeOnly}
        <div transition:slide={{ duration: 150 }}>
          <NoticeRow status="warn">
            A rebuild adds keyword search, so exact names and error strings rank beside meaning.
            {#snippet action()}
              <Button size="sm" variant="outline" onclick={() => void backfill('reindex')} disabled={!servicesUp}>Rebuild index</Button>
            {/snippet}
          </NoticeRow>
        </div>
      {/if}
      <QueryState query={collectionsQuery} errorTitle="Could not load the collections" errorVariant="compact">
        {#snippet skeleton()}
          <div class="px-5 py-4"><SkeletonRows rows={3} avatar /></div>
        {/snippet}
        {#snippet children(collections)}
          {#each brokenFirst(collections) as c (c.id)}
            <CollectionRow
              col={c}
              spaces={rag?.spaces ?? []}
              index={indexOf(c.id)}
              rebuild={canRebuild ? () => void backfill('reindex') : undefined}
            />
          {/each}
        {/snippet}
      </QueryState>
      <!-- Creating a collection while the list is unknown risks a duplicate of
           one that is already there — hold the button until we know what exists. -->
      <SettingsRow label="New collection" hint="Name it after what it holds." as="label">
        <Input size="sm" class="w-[280px]" bind:value={name} placeholder="Sales playbook" onkeydown={(e) => e.key === 'Enter' && void create()} />
        <Button size="md" variant="outline" onclick={() => void create()} disabled={busy || !name.trim() || collectionsQuery.data === undefined}>
          Create collection
        </Button>
      </SettingsRow>
    </SettingsGroup>
  </SettingsSection>

  <SettingsSection title="Index" description="The services that build and search the index.">
    <SettingsGroup>
      <!-- A failure here must NOT paint the status rows red — "we could not
           ask" is not "the vector store is down". -->
      {#if ragPending}<div class="px-5 py-4"><SkeletonRows rows={3} /></div>{/if}
      {#if ragFailed}
        <div transition:slide={{ duration: 150 }}>
          <QueryError variant="compact" error={ragQuery.error} title="Could not load retrieval status" onRetry={() => void ragQuery.refetch()} />
        </div>
      {/if}
      {#if rag}
        <SettingsRow label="Vector store" hint="Where the index is kept.">
          <RowStatus status={rag.health.qdrant ? 'ok' : 'danger'}>{rag.health.qdrant ? 'Running' : 'Down'}</RowStatus>
        </SettingsRow>
        <SettingsRow
          label="Embeddings"
          info="To change the model, set TALARIA_EMBED_MODEL, restart the embeddings container, then rebuild the index."
        >
          {#snippet hint()}
            {#if rag.upgrade?.embed}
              <span class="font-mono text-xs">{rag.upgrade.embed.modelId}</span> · <span class="tabular-nums">{rag.upgrade.embed.dim.toLocaleString()}</span> dimensions
            {:else}
              Turns text into the vectors search compares.
            {/if}
          {/snippet}
          <RowStatus status={rag.health.embeddings ? 'ok' : 'danger'}>{rag.health.embeddings ? 'Running' : 'Down'}</RowStatus>
        </SettingsRow>
        <SettingsRow label="Backfill">
          {#snippet hint()}
            {#if rag.backfill.state === 'running'}
              Indexing the records that already exist.
            {:else if rag.backfill.state === 'error'}
              The last run failed: {rag.backfill.error}
            {:else if rag.backfill.state === 'done' && rag.backfill.counts}
              The last run indexed {counts(rag.backfill.counts)}.
            {:else}
              Indexes the records that existed before retrieval was on.
            {/if}
          {/snippet}
          {#if rag.backfill.state === 'running'}
            <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
              <WaitingMark site="admin/retrieval-backfill" size={14} />Running
            </span>
          {:else if rag.backfill.state === 'error'}
            <RowStatus status="danger">Failed</RowStatus>
          {/if}
          <Button size="sm" variant="outline" onclick={() => void backfill()} disabled={rebuilding || rag.backfill.state === 'running' || !servicesUp}>
            Run backfill
          </Button>
        </SettingsRow>
      {/if}
    </SettingsGroup>
  </SettingsSection>

  {#if rag}<RerankSection {rag} />{/if}
</div>
