<script lang="ts">
  import { BookOpen, FileText, Gem } from '@lucide/svelte'
  import { p } from '@/router'
  import { attachmentUrl, humanSize, isImage, type Attachment } from '@/lib/attachments'

  // How attachments render inside a message: images inline (click to open),
  // files as download chips, knowledge/artifact refs as link chips.
  let { items }: { items: Attachment[] } = $props()

  const refs = $derived((items ?? []).filter((a) => a.refType))
  const images = $derived((items ?? []).filter((a) => !a.refType && isImage(a.mime)))
  const files = $derived((items ?? []).filter((a) => !a.refType && !isImage(a.mime)))
</script>

{#if items?.length}
  <div class="mt-2 space-y-2">
    {#if images.length > 0}
      <div class="flex flex-wrap gap-2">
        {#each images as a (a.id)}
          <a href={attachmentUrl(a.id)} target="_blank" rel="noreferrer">
            <img src={attachmentUrl(a.id)} alt={a.filename} class="max-h-48 rounded-md border border-line object-cover" />
          </a>
        {/each}
      </div>
    {/if}
    {#each refs as a (a.id)}
      <a
        href={a.refType === 'kb-doc' ? p('/knowledge') : p('/artifacts')}
        class="inline-flex items-center gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 font-sans text-xs text-fg transition-colors hover:border-line-strong"
        title={a.refType === 'kb-doc' ? 'Attached knowledge doc' : 'Attached artifact'}
      >
        <!-- Ref chips: knowledge docs get the book, artifacts the gem. -->
        {#if a.refType === 'kb-doc'}
          <BookOpen size={14} class="shrink-0 text-muted" />
        {:else}
          <Gem size={14} class="shrink-0 text-muted" />
        {/if}
        <span class="max-w-48 truncate">{a.filename}</span>
      </a>
    {/each}
    {#each files as a (a.id)}
      <a
        href={attachmentUrl(a.id)}
        target="_blank"
        rel="noreferrer"
        class="inline-flex items-center gap-2 rounded-md border border-line bg-raised px-2.5 py-1.5 font-sans text-xs text-fg transition-colors hover:border-line-strong"
      >
        <FileText size={14} class="text-muted" />
        <span class="max-w-48 truncate">{a.filename}</span>
        <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{humanSize(a.size)}</span>
      </a>
    {/each}
  </div>
{/if}
