<script lang="ts">
  import { FileText, Paperclip } from '@lucide/svelte'
  import { openFileViewer } from '@/lib/file-viewer.svelte'

  // One inline attachment in a KB doc body (TALA-2). The markdown body carries
  // `![name|size](upload:<id>)` / `[name|size](upload:<id>)`; the pipeline
  // leaves a placeholder span and Markdown.svelte hydrates it with this
  // component. Images render the bytes inline (click opens the file viewer);
  // other files render a labeled chip — filename and size ride the markdown,
  // so the read surface needs no fetches to draw it. Access is the server's
  // call: /api/uploads/{id} serves bytes only to viewers who can read a
  // document that references the upload.
  let { id, filename, size }: { id: string; filename: string; size: string } = $props()

  const url = $derived(`/api/uploads/${encodeURIComponent(id)}`)
  const image = $derived(/\.(png|jpe?g|gif|webp|avif)$/i.test(filename))

  function open() {
    openFileViewer({
      url,
      title: filename,
      // Mime rides the response headers; the viewer previews from what it can
      // read and falls back to the honest card otherwise.
      contentType: null,
      sizeBytes: null,
    })
  }
</script>

{#if image}
  <button type="button" class="block max-w-full" title="{filename} — open" onclick={open}>
    <img src={url} alt={filename} class="my-2 max-h-96 rounded-lg border border-line" loading="lazy" />
  </button>
{:else}
  <button
    type="button"
    onclick={open}
    class="my-1 inline-flex max-w-full items-center gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 font-sans text-xs text-fg transition-colors hover:border-line-strong"
    title="Open {filename}"
  >
    {#if /\.pdf$/i.test(filename)}<Paperclip size={14} class="shrink-0 text-muted" />{:else}<FileText size={14} class="shrink-0 text-muted" />{/if}
    <span class="max-w-48 truncate">{filename}</span>
    {#if size}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{size}</span>{/if}
  </button>
{/if}