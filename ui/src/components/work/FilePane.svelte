<script lang="ts">
  // ONE FILE OF A WORK SESSION, in the pane beside the stream: a single header
  // row that reads the same for every kind of file, and the file under it.
  //
  //   [type] TYPE LABEL  Title ……………  (Read|Edit)  💬  ↗  ⤢  ⋯  ✕
  //
  // The row is the ONLY chrome above the file. A Talaria artifact's own
  // toolbar is laid INTO it (ArtifactEditor's pane mode: the title, which still
  // renames in Edit, the Read/Edit switch and its More menu), and a Google file
  // embeds bare, so nothing stacks a second row naming the same file.
  //
  // IT FILLS THE BOX IT IS GIVEN: the caller owns the column. Fullscreen lifts
  // the whole pane, header included, over the app; Esc puts it back.
  import { ExternalLink, Maximize2, MessageSquareText, Minimize2, X } from '@lucide/svelte'
  import GoogleFilePane from '@/components/chat/GoogleFilePane.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import IconButton from '@/components/ui/IconButton.svelte'
  import { cn } from '@/lib/cn'
  import { isEditableEmbed } from '@/lib/google-embed'
  import { fileGlyph, fileTitle, fileTypeLabel, googleMime, googleOpenHref, type SessionFile } from '@/lib/session-files'
  import { p } from '@/router'
  import ArtifactEditor, { type PaneParts } from '@/routes/app/ArtifactEditor.svelte'
  import ArtifactComments from './ArtifactComments.svelte'
  import FileGlyph from './FileGlyph.svelte'

  let {
    file,
    onClose,
    onDeleted,
  }: {
    file: SessionFile
    /** Close the tab — the strip owns it, so the pane asks. */
    onClose: () => void
    /** The artifact was deleted from inside the editor. */
    onDeleted: () => void
  } = $props()

  let fullscreen = $state(false)
  let showComments = $state(false)

  const mime = $derived(googleMime(file))
  const title = $derived(fileTitle(file))
  // Files for a Talaria artifact (the same address the old plan header linked),
  // the real Google editor for a Google file — in a new tab, because the embed
  // can be signed out in ways this side cannot see (GoogleFilePane's note).
  const openHref = $derived(file.kind === 'artifact' ? `${p('/artifacts')}?a=${file.id}` : googleOpenHref(file))
  const readOnly = $derived(file.kind === 'google' && !isEditableEmbed(mime))
  const iconLink = buttonClasses({ variant: 'ghost', size: 'xs', className: 'h-7 w-7 shrink-0 px-0' })
</script>

<svelte:window
  onkeydown={(e) => {
    if (fullscreen && e.key === 'Escape') fullscreen = false
  }}
/>

{#snippet row(parts: PaneParts | null)}
  <div class="flex h-11 shrink-0 items-center gap-2 border-b border-line-subtle pl-4 pr-2">
    <FileGlyph glyph={fileGlyph(file)} class="text-muted" />
    <!-- The type in the chrome voice. Below `sm` the glyph says it alone, so
         the title keeps the room. -->
    <span class="hidden shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim sm:inline">
      {fileTypeLabel(file)}
    </span>
    {#if parts}
      {@render parts.title()}
    {:else}
      <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg" {title}>{title}</span>
    {/if}
    {#if readOnly}
      <!-- Said plainly rather than left for someone to discover by typing into
           a frame that will not take it: a deck and a PDF are editable only in
           Google. -->
      <span class="hidden shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim md:inline">
        read-only here
      </span>
    {/if}
    {#if parts && !showComments}{@render parts.tools()}{/if}
    <span class="flex shrink-0 items-center">
      {#if file.kind === 'artifact'}
        <IconButton
          size="sm"
          title={showComments ? 'Back to the document' : 'Comments'}
          active={showComments}
          onclick={() => (showComments = !showComments)}
        >
          <MessageSquareText size={14} />
        </IconButton>
      {/if}
      <a
        href={openHref}
        target={file.kind === 'google' ? '_blank' : undefined}
        rel={file.kind === 'google' ? 'noopener noreferrer' : undefined}
        class={cn(iconLink, 'dither-fill')}
        title={file.kind === 'google' ? 'Open in Google, in a new tab' : 'Open in Files'}
        aria-label={file.kind === 'google' ? 'Open in Google' : 'Open in Files'}
      >
        <ExternalLink size={14} />
      </a>
      <IconButton
        size="sm"
        title={fullscreen ? 'Exit fullscreen (Esc)' : 'Fullscreen'}
        onclick={() => (fullscreen = !fullscreen)}
      >
        {#if fullscreen}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
      </IconButton>
      {#if parts}{@render parts.menu()}{/if}
      <IconButton size="sm" title={`Close ${title}`} onclick={onClose}>
        <X size={14} />
      </IconButton>
    </span>
  </div>
{/snippet}

{#snippet comments()}
  <ArtifactComments artifactId={file.id} onClose={() => (showComments = false)} />
{/snippet}

<div class={cn('flex min-h-0 min-w-0 flex-1 flex-col', fullscreen && 'fixed inset-0 z-50 bg-surface')}>
  {#if file.kind === 'artifact'}
    <!-- Keyed on the id ALONE: a write tool landing invalidates the artifact
         query instead, so the editor keeps its scroll and any unsaved edit
         while the new body arrives. -->
    {#key file.id}
      <ArtifactEditor id={file.id} {onDeleted} header={row} override={showComments ? comments : undefined} />
    {/key}
  {:else}
    {@render row(null)}
    <GoogleFilePane bare fileId={file.id} {mime} {title} />
  {/if}
</div>
