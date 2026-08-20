<script lang="ts">
  import ChatComposer from '@/components/chat/ChatComposer.svelte'
  import type { ChatComposerHandle } from '@/components/chat/chat-composer'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import PendingAttachments from '@/components/chat/PendingAttachments.svelte'
  import EmojiButton from '@/components/chat/EmojiButton.svelte'
  import { cn } from '@/lib/cn'
  import { uploadFile, type Attachment } from '@/lib/attachments'
  import type { Mentionable } from '@/components/chat/mentions.svelte'

  // The channel composer (channel-view.tsx's internal `Composer`): the
  // Slack-shaped rich editor (ChatComposer.svelte) plus attachment chips.
  // Files paste and drop straight in — images from the clipboard, files from
  // the desktop — uploading immediately as pending chips.
  let {
    channelName,
    placeholder,
    mentionables,
    onSend,
  }: {
    channelName: string
    placeholder?: string
    mentionables: Mentionable[]
    onSend: (text: string, attachments: Attachment[]) => Promise<void>
  } = $props()

  let attachments = $state<Attachment[]>([])
  let empty = $state(true)
  let dragging = $state(false)
  let editorRef = $state<ChatComposerHandle | null>(null)

  const uploadAll = (files: Iterable<File>) => {
    for (const f of files) {
      void uploadFile(f).then((r) => {
        if ('id' in r) attachments.push(r)
      })
    }
  }

  const submit = (markdown: string) => {
    if (!markdown && attachments.length === 0) return
    const atts = attachments
    attachments = []
    editorRef?.clear()
    void onSend(markdown, atts)
  }
</script>

<div class="pointer-events-auto relative px-6 pb-6">
  <!-- The composer panel (spec §7): #141312 body, strong 1px border,
      radius 8, 8px padding/gap, matte float shadow. -->
  <div
    class={cn(
      'flex flex-col gap-2 rounded-lg border border-line-strong bg-panel p-2 shadow-[var(--theme-shadow-2)] transition-colors',
      dragging && 'border-accent bg-accent-soft',
    )}
    ondragover={(e) => {
      if (e.dataTransfer?.types.includes('Files')) {
        e.preventDefault()
        dragging = true
      }
    }}
    ondragleave={() => (dragging = false)}
    ondrop={() => (dragging = false)}
    role="group"
  >
    <PendingAttachments items={attachments} onRemove={(id) => (attachments = attachments.filter((a) => a.id !== id))} />
    <ChatComposer
      bind:this={editorRef}
      placeholder={placeholder ??
        `What would you like #${channelName} to work on? @mention an agent to bring it in`}
      {mentionables}
      onSubmit={submit}
      onFiles={uploadAll}
      onEmptyChange={(v) => (empty = v)}
      canSend={!empty || attachments.length > 0}
    >
      <!-- No single selected agent in a channel — the generic ask (the
           placeholder above), with the @mention affordance kept discoverable.
           The rail's last word is the send tile ChatComposer pins itself. -->
      {#snippet leftControls()}
        <AttachButton onAttach={(a) => attachments.push(a)} />
        <EmojiButton onPick={(ch) => editorRef?.insertText(ch)} />
      {/snippet}
    </ChatComposer>
  </div>
</div>
