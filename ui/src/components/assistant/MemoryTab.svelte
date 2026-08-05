<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { type Assistant } from '@/lib/assistant'

  let { assistant }: { assistant: Assistant } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['assistant-memory', assistant.id],
    // The old shape leaned on the server's `{ error }` field surviving inside a
    // non-2xx body. A non-2xx WITHOUT that field (a 502 from a proxy, `{}`) fell
    // through as `{ content: undefined }` and drew "Nothing remembered yet" —
    // an emptiness claim about a document nobody actually read.
    queryFn: async (): Promise<{ content: string }> => ({
      content: (await getJson<{ content?: string }>(`/api/memory/${assistant.id}`)).content ?? '',
    }),
  }))
  let editing = $state(false)
  let busy = $state(false)

  const save = async (md: string) => {
    busy = true
    try {
      await fetch(`/api/memory/${assistant.id}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: md }),
      })
      await qc.invalidateQueries({ queryKey: ['assistant-memory', assistant.id] })
    } finally {
      busy = false
    }
  }
</script>

{#if query.isLoading}
  <div class="space-y-4">
    <SectionHeader
      class="mb-0"
      title="Memory"
      info="What it remembers about you and your work. It updates this itself as you go, and you can edit or prune it any time. Every save is snapshotted, so nothing is ever lost."
    />
    <div aria-hidden="true" class="max-h-72 rounded-lg border border-line p-3">
      <SkeletonRows rows={8} />
    </div>
  </div>
{:else if query.isError && query.data === undefined}
  <!-- A stopped assistant is an explainable state, not a fault — keep its own
       sentence. Anything else shows the server's reason. -->
  {#if assistant.running}
    <QueryError error={query.error} title="Memory unavailable" onRetry={() => void query.refetch()} />
  {:else}
    <EmptyState icon="◌" title="Memory unavailable" hint="Start your assistant to read its memory." />
  {/if}
{:else}
  <div class="space-y-4">
    <SectionHeader
      class="mb-0"
      title="Memory"
      info="What it remembers about you and your work. It updates this itself as you go, and you can edit or prune it any time. Every save is snapshotted, so nothing is ever lost."
    />
    {#if query.data?.content}
      <div class="max-h-72 overflow-y-auto rounded-lg border border-line p-3 text-sm">
        <Markdown children={query.data.content} />
      </div>
    {:else}
      <EmptyState icon="◌" title="Nothing remembered yet" hint="It writes things down as you work together." />
    {/if}
    <Button size="sm" variant="outline" onclick={() => (editing = true)}>
      Open editor
    </Button>
    {#if editing}
      <InternalEditorModal
        open
        onClose={() => (editing = false)}
        title={`${assistant.displayName} · Memory`}
        subtitle="Your assistant maintains this itself; your edits are snapshotted and revertible."
        value={query.data?.content ?? ''}
        editable
        saving={busy}
        onSave={save}
        history={{ kind: 'memory', id: assistant.id }}
        muse={{ kind: 'memory', context: `The memory of ${assistant.displayName}, a personal AI assistant.` }}
      />
    {/if}
  </div>
{/if}
