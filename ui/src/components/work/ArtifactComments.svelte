<script lang="ts">
  // COMMENTS ON A TALARIA DOCUMENT, filling the box they are given. The work
  // pane's column is too narrow to split again, so the threads swap in for
  // the document rather than standing beside it; the file's own header row
  // stays above them. The read is here and nowhere else, keyed the way
  // KbCommentsPanel invalidates it after a write.
  import { createQuery } from '@tanstack/svelte-query'
  import { getJson } from '@/lib/fetch-json'
  import { useSession } from '@/lib/session'
  import KbCommentsPanel from '@/routes/app/KbCommentsPanel.svelte'
  import type { KbComment } from '@/routes/app/knowledge.svelte'

  let { artifactId, onClose }: { artifactId: string; onClose: () => void } = $props()

  const session = useSession()
  const commentsQuery = createQuery(() => ({
    queryKey: ['artifact-comments', artifactId],
    queryFn: (): Promise<{ comments: KbComment[] }> =>
      getJson<{ comments: KbComment[] }>(`/api/artifacts/${artifactId}/comments`),
  }))
</script>

<KbCommentsPanel
  fill
  docId={artifactId}
  basePath="/api/artifacts"
  queryKey="artifact-comments"
  comments={commentsQuery.data?.comments ?? []}
  loadFailed={commentsQuery.isError}
  loadError={commentsQuery.error}
  onRetryLoad={() => void commentsQuery.refetch()}
  meId={session.data?.id ?? null}
  docOwnerId={null}
  pendingQuote={null}
  onQuoteConsumed={() => {}}
  {onClose}
/>
