<script lang="ts">
  import { ExternalLink } from '@lucide/svelte'
  import { embedUrl, isEditableEmbed, openUrl } from '@/lib/google-embed'

  // A Google file in the document pane, with Google's own editor and Google's
  // own presence doing the collaboration — which is the entire reason to embed
  // rather than to mirror the file into an artifact and fight over which copy
  // is real.
  //
  // THE FRAME CAN FAIL AND WE CANNOT SEE IT. The embed is cross-origin, so a
  // browser that blocks third-party cookies shows Google's sign-in wall inside
  // the frame and this component cannot read that, or anything else, out of it.
  // There is no honest "did it work" signal to branch on. So the escape hatch is
  // not an error state that appears when something goes wrong — it is a
  // permanent control that is always there, because the failure it answers is
  // invisible from this side.
  let {
    fileId,
    mime = null,
    title = null,
  }: { fileId: string; mime?: string | null; title?: string | null } = $props()

  const src = $derived(embedUrl(fileId, mime))
  const href = $derived(openUrl(fileId, mime))
  const editable = $derived(isEditableEmbed(mime))
</script>

<div class="flex min-h-0 flex-1 flex-col">
  <div class="flex shrink-0 items-center gap-2 border-b border-line-subtle px-4 py-1.5">
    <span class="truncate font-sans text-xs text-muted" title={title ?? fileId}>
      {title ?? 'Google file'}
    </span>
    {#if !editable}
      <!-- Said plainly rather than left for someone to discover by typing into
           a frame that will not take it. A deck and a PDF are readable here and
           editable only in Google. -->
      <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">read-only here</span>
    {/if}
    <a
      {href}
      target="_blank"
      rel="noopener noreferrer"
      class="ml-auto flex shrink-0 items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
      title="Open in Google, in a new tab"
    >
      <ExternalLink size={12} />
      Open
    </a>
  </div>
  <!-- `allow-same-origin` is required for Google's editor to reach its own
       session; without it the frame is signed out by construction. The sandbox
       still withholds top-level navigation, so the embed cannot move the tab
       out from under someone. -->
  <iframe
    {src}
    title={title ?? 'Google file'}
    class="min-h-0 flex-1 border-0 bg-surface"
    sandbox="allow-same-origin allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox"
    referrerpolicy="no-referrer-when-downgrade"
  ></iframe>
</div>
