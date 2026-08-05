<script lang="ts">
  import { BookOpen, FileText, Gem, X } from '@lucide/svelte'
  import { fade, slide, QUICK } from '@/lib/motion'
  import { attachmentUrl, isImage, type Attachment } from '@/lib/attachments'

  // The pending-attachments strip above the composer's prompt well (with
  // remove buttons) — raised chips on the composer panel.
  let { items, onRemove }: { items: Attachment[]; onRemove: (id: string) => void } = $props()
</script>

{#if items.length}
  <!-- The strip slides open/closed (composer height change); individual chips
      fade only when added to or removed from an already-open strip — local
      transitions keep first-chip/last-chip from double-animating. -->
  <div transition:slide={{ duration: 150 }} class="flex flex-wrap gap-2">
    {#each items as a (a.id)}
      <div
        in:fade={{ duration: 150 }}
        out:fade={QUICK}
        class="flex items-center gap-2 rounded-md border border-line bg-raised px-2 py-1 font-sans text-xs"
      >
        {#if a.refType}
          <!-- Ref chips: knowledge docs get the book, artifacts the gem. -->
          {#if a.refType === 'kb-doc'}
            <BookOpen size={14} class="shrink-0 text-muted" />
          {:else}
            <Gem size={14} class="shrink-0 text-muted" />
          {/if}
        {:else if isImage(a.mime)}
          <img src={attachmentUrl(a.id)} alt={a.filename} class="h-6 w-6 rounded object-cover" />
        {:else}
          <FileText size={14} class="text-muted" />
        {/if}
        <span class="max-w-32 truncate text-fg">{a.filename}</span>
        <button type="button" onclick={() => onRemove(a.id)} class="text-muted hover:text-[color:var(--theme-danger)]">
          <X size={13} />
        </button>
      </div>
    {/each}
  </div>
{/if}
