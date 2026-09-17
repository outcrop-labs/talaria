<script lang="ts">
  import { BookOpen, FileText, Gem } from '@lucide/svelte'
  import { p } from '@/router'
  import { attachmentUrl, humanSize, isImage, type Attachment } from '@/lib/attachments'
  import { openFileViewer } from '@/lib/file-viewer.svelte'

  // How attachments render inside a message: images inline (click opens the
  // file modal), files as chips that open the same modal, knowledge/artifact
  // refs as in-app links. Never target=_blank — that left desktop users in a
  // browser tab with no way to save.
  let { items }: { items: Attachment[] } = $props()

  const refs = $derived((items ?? []).filter((a) => a.refType))
  const images = $derived((items ?? []).filter((a) => !a.refType && isImage(a.mime)))
  const files = $derived((items ?? []).filter((a) => !a.refType && !isImage(a.mime)))

  const openFile = (a: Attachment) =>
    openFileViewer({
      url: attachmentUrl(a.id),
      title: a.filename,
      contentType: a.mime,
      sizeBytes: a.size,
    })
</script>

{#if items?.length}
  <div class="mt-2 space-y-2">
    {#if images.length > 0}
      <div class="flex flex-wrap gap-2">
        {#each images as a (a.id)}
          <button type="button" class="block" title={a.filename} onclick={() => openFile(a)}>
            <img src={attachmentUrl(a.id)} alt={a.filename} class="max-h-48 rounded-md border border-line object-cover" />
          </button>
        {/each}
      </div>
    {/if}
    {#each refs as a (a.id)}
      <!-- Deep-linked, not index-linked. A chip that opened /artifacts left the
           reader to find the document themselves in a list that may be hundreds
           long — which is the same dead end as an agent describing where a
           report is filed instead of pointing at it. Both surfaces take an id:
           /artifacts?a=<id> and /knowledge?doc=<id>. -->
      <a
        href={a.refType === 'kb-doc' ? `${p('/knowledge')}?doc=${encodeURIComponent(a.id)}` : `${p('/artifacts')}?a=${encodeURIComponent(a.id)}`}
        class="inline-flex items-center gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 font-sans text-xs text-fg transition-colors hover:border-line-strong"
        title={a.refType === 'kb-doc' ? 'Open this knowledge doc' : 'Open this artifact'}
      >
        {#if a.refType === 'kb-doc'}
          <BookOpen size={14} class="shrink-0 text-muted" />
        {:else}
          <Gem size={14} class="shrink-0 text-muted" />
        {/if}
        <span class="max-w-48 truncate">{a.filename}</span>
      </a>
    {/each}
    {#each files as a (a.id)}
      <button
        type="button"
        onclick={() => openFile(a)}
        class="inline-flex items-center gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 font-sans text-xs text-fg transition-colors hover:border-line-strong"
      >
        <FileText size={14} class="text-muted" />
        <span class="max-w-48 truncate">{a.filename}</span>
        <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{humanSize(a.size)}</span>
      </button>
    {/each}
  </div>
{/if}
