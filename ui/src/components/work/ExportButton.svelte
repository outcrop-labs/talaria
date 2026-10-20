<script lang="ts">
  // EXPORT THIS SESSION — the thread as a document you can keep, share with
  // somebody who does not use Talaria, or hand to a client as a .docx. The
  // markdown is `lib/transcript`; this is the one button that reads the
  // conversation, saves the result as an artifact, and offers the Word file.
  //
  // `getJson`, NOT `loadConversation`: that helper is deliberately
  // non-throwing (chat-view drives it from effects and polls with no rejection
  // handler, see its comment) and answers `null` for every failure — a 403, a
  // 500 and a deleted thread all alike. An export that quietly produced an
  // empty transcript out of a failed read is the one outcome worse than an
  // error, so this path wants the rejection.
  import { Download } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { createArtifact, officeExportHref } from '@/lib/artifacts'
  import type { Conversation, StoredMessage } from '@/lib/conversations.svelte'
  import { downloadFile } from '@/lib/download-file'
  import { getJson } from '@/lib/fetch-json'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import { transcriptTitle, transcriptToMarkdown } from '@/lib/transcript'

  let {
    conversationId,
    agentLabel,
  }: {
    /** The session being exported. Null renders NOTHING: there is no useful
     *  disabled reading of this — a transcript needs a thread. */
    conversationId: string | null
    /** What the agent's turns are labelled, which the harness already knows
     *  (and which outlives the agent that spoke). */
    agentLabel?: string
  } = $props()

  let busy = $state(false)
  /** The artifact the last export made, so the Word file can be offered
   *  WITHOUT a second read or a second row. The toast carries the link to the
   *  document (toasts take an href, not actions); the follow-up control below
   *  is the second half of the same offer. */
  let made = $state<{ id: string; title: string } | null>(null)
  let downloading = $state(false)
  // A follow-up belongs to the session it came out of: switching sessions
  // retires it rather than offering the previous thread's file from this one.
  $effect(() => {
    void conversationId
    made = null
  })

  const run = async () => {
    const id = conversationId
    if (!id || busy) return
    busy = true
    try {
      const read = await getJson<{ conversation: Conversation; messages: StoredMessage[] }>(
        `/api/conversations/${id}`,
      )
      const artifact = await createArtifact({
        kind: 'doc',
        title: transcriptTitle(read.conversation),
        // Tools in, reasoning out: a transcript is a record of the
        // conversation, and the tool line is the half somebody auditing the
        // session actually asks for.
        body: transcriptToMarkdown(read.conversation, read.messages ?? [], {
          agentLabel,
          includeTools: true,
        }),
      })
      made = { id: artifact.id, title: artifact.title }
      pushToast({
        title: 'Transcript saved',
        body: artifact.title,
        href: `/artifacts?a=${artifact.id}`,
        tone: 'success',
      })
    } catch (e) {
      toastError('Could not export the transcript', e)
    } finally {
      busy = false
    }
  }

  const word = async () => {
    const m = made
    if (!m || downloading) return
    downloading = true
    try {
      await downloadFile(officeExportHref(m.id, 'docx'), `${m.title}.docx`)
    } catch (e) {
      toastError('Could not download the Word file', e)
    } finally {
      downloading = false
    }
  }
</script>

{#if conversationId}
  <IconButton size="sm" title="Export transcript" disabled={busy} onclick={() => void run()}>
    <Download size={14} />
  </IconButton>
  {#if made}
    <!-- The second action on the export that just happened. Labelled, because
         a format is not a verb placement can imply. -->
    <Button
      variant="ghost"
      size="xs"
      disabled={downloading}
      title="Download the saved transcript as a Word file"
      onclick={() => void word()}
    >
      {downloading ? 'Downloading' : 'Word (.docx)'}
    </Button>
  {/if}
{/if}
