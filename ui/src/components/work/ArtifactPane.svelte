<script lang="ts">
  // THE DOCUMENT PANE of a work session: the thing the session is about, beside
  // the stream that is working on it. Three documents can stand here — a
  // Talaria artifact the agent opened or wrote, a Google file pinned to the
  // session, or whatever the caller hands in as `children` (the session's
  // living document, or an empty state when there is no session yet) — plus the
  // comment threads, which swap IN over the document rather than sitting beside
  // it.
  //
  // IT FILLS THE BOX IT IS GIVEN. The caller owns the column: how wide it is,
  // which breakpoint it appears at, whether it carries a divider. Pass that
  // through `class` — nothing in here assumes a width, and nothing should,
  // because the same pane is worth standing in a dock or a sheet later.
  import type { Snippet } from 'svelte'
  import { createQuery } from '@tanstack/svelte-query'
  import GoogleFilePane from '@/components/chat/GoogleFilePane.svelte'
  import { alert, prompt } from '@/components/ui/confirm.svelte'
  import { cn } from '@/lib/cn'
  import { loadConversation, setPinnedFiles, type PinnedFile } from '@/lib/conversations.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { parseGoogleFileUrl } from '@/lib/google-embed'
  import { useSession } from '@/lib/session'
  import ArtifactEditor from '@/routes/app/ArtifactEditor.svelte'
  import KbCommentsPanel from '@/routes/app/KbCommentsPanel.svelte'
  import type { KbComment } from '@/routes/app/knowledge.svelte'

  let {
    conversationId,
    artifactId,
    livingDocId,
    onCloseArtifact,
    children,
    class: className,
  }: {
    /** The work session this pane belongs to — null before the first turn
     *  saves one, which is why the pin controls are hidden until then. */
    conversationId: string | null
    /** The artifact the session last touched, reported by the stream. It wins
     *  the pane: the agent opened or wrote a file, so the file is what to show. */
    artifactId: string | null
    /** The session's living document, resolved by whoever renders it below. The
     *  pane only needs its id, to hang comments off it when no artifact is open. */
    livingDocId: string | null
    /** Both the Close control and a deletion from inside the editor — the pane
     *  does not own `artifactId`, so it asks rather than clears. */
    onCloseArtifact: () => void
    /** The default document, when nothing else is claiming the pane. The caller
     *  renders it because the choice is the caller's: a living document for a
     *  live session, an empty state for a surface that has none yet. */
    children?: Snippet
    class?: string
  } = $props()

  const session = useSession()
  const quiet =
    'flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg'

  // A GOOGLE FILE PINNED TO THIS SESSION, if any — the pane's Tier A. Held per
  // session on the conversation row (`pinned_files`), so reopening a session
  // puts the same file back in front of you, and a collaborator who opens it
  // sees the file the session is about rather than an empty pane.
  let pinnedGoogle = $state<PinnedFile | null>(null)
  $effect(() => {
    const id = conversationId
    if (!id) {
      pinnedGoogle = null
      return
    }
    // Read through the detail endpoint rather than the list: `pinned_files` is
    // a per-conversation column and the rail's rows do not carry it.
    void loadConversation(id).then((d) => {
      if (conversationId !== id) return
      pinnedGoogle = (d?.conversation.pinnedFiles ?? []).find((f) => f.kind === 'google') ?? null
    })
  })
  // PASTE A LINK. The smallest honest way in: people already have the URL, and
  // a Drive browser is a bigger surface than this pane needs to prove itself.
  // The type is read from the URL's own shape — a link that is not a Google
  // file is refused here rather than pinned into a blank frame.
  const pinGoogle = async () => {
    const id = conversationId
    if (!id) return
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
    const file: PinnedFile = { kind: 'google', id: hit.id, ...(hit.mime ? { mime: hit.mime } : {}) }
    if (await setPinnedFiles(id, [file])) pinnedGoogle = file
  }
  const unpinGoogle = () => {
    const id = conversationId
    pinnedGoogle = null
    if (id) void setPinnedFiles(id, [])
  }

  // COMMENTS ON THE DOCUMENT IN THE PANE. The pane is narrow, so they swap in
  // rather than sitting beside the editor — splitting an already-split stage
  // again would give neither half enough room to be worth having.
  let showComments = $state(false)
  // The artifact the comments belong to: whatever the pane is actually showing.
  // A pinned Google file has Google's own comments inside the embed, so this is
  // null there and the control is hidden.
  const commentTargetId = $derived(artifactId ?? livingDocId)
  const commentsQuery = createQuery(() => {
    const id = commentTargetId
    return {
      queryKey: ['artifact-comments', id],
      enabled: !!id && showComments,
      queryFn: (): Promise<{ comments: KbComment[] }> =>
        getJson<{ comments: KbComment[] }>(`/api/artifacts/${id}/comments`),
    }
  })

</script>

<div class={cn('flex min-w-0 flex-1 flex-col', className)}>
  <div class="flex h-12 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Document</span>
    {#if commentTargetId && !pinnedGoogle}
      <button type="button" class={`${quiet} ml-auto`} onclick={() => (showComments = !showComments)}>
        {showComments ? 'Document' : 'Comments'}
      </button>
    {/if}
    {#if artifactId}
      <button type="button" class={quiet} onclick={onCloseArtifact}>Close</button>
    {:else if pinnedGoogle}
      <button type="button" class={`${quiet} ml-auto`} onclick={unpinGoogle}>Unpin</button>
    {:else if conversationId}
      <button type="button" class={`${quiet} ml-auto`} onclick={() => void pinGoogle()}>Open a Google file</button>
    {/if}
  </div>
  {#if showComments && commentTargetId}
    <!-- Swapped IN rather than beside: the pane owns a column of the stage and
         splitting it again would leave neither half usable. A pinned Google
         file never gets here — Google's own comments live inside the embed. -->
    <KbCommentsPanel
      docId={commentTargetId}
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
      onClose={() => (showComments = false)}
    />
  {:else if artifactId}
    <!-- The agent opened or wrote a DIFFERENT file, so the pane follows the
         work. Keyed on the id ALONE, not on a refresh counter: a tool landing
         invalidates the artifact query instead, so the editor keeps its scroll
         and any unsaved edit while the new body arrives. -->
    {#key artifactId}
      <div class="min-h-0 flex-1 overflow-hidden">
        <ArtifactEditor id={artifactId} onDeleted={onCloseArtifact} />
      </div>
    {/key}
  {:else if pinnedGoogle}
    <!-- TIER A: the real Google editor, with Google's own presence doing the
         collaboration. Pinned to the session, so it outlives a reload and a
         collaborator sees the same file. -->
    <GoogleFilePane
      fileId={pinnedGoogle.id}
      mime={pinnedGoogle.mime ?? null}
      title={pinnedGoogle.title ?? null}
    />
  {:else if children}
    {@render children()}
  {/if}
</div>
