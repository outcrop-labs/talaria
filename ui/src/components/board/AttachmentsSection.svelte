<script lang="ts">
  import { BookOpen, FileText, Gem, X } from '@lucide/svelte'
  import { p } from '@/router'
  import AttachButton from '@/components/chat/AttachButton.svelte'
  import { attachmentUrl, humanSize, isImage, splitAttachments, type Attachment } from '@/lib/attachments'
  import { updateTask } from '@/lib/boards.svelte'
  import { listStagger } from '@/lib/motion'
  import type { Task } from '@/lib/task-const'
  import Section from './Section.svelte'

  // Files + knowledge/artifact refs pinned to the ticket. Same chips as chat;
  // every change saves immediately (the list on the ticket IS the state).
  let { task, canEdit, onSaved }: { task: Task; canEdit: boolean; onSaved: () => void } = $props()

  const items = $derived(task.attachments ?? [])
  const save = (next: Attachment[]) => {
    void updateTask(task.id, splitAttachments(next)).then(onSaved)
  }
</script>

{#if items.length || canEdit}
  <Section label="Attachments">
    {#if items.length > 0}
      <div class="mb-2 flex flex-wrap gap-2" use:listStagger>
        {#each items as a (a.id)}
          <span class="inline-flex items-center gap-2 rounded-md border border-line bg-raised/50 px-2.5 py-1.5 font-sans text-xs">
            {#if a.refType}
              <a
                href={a.refType === 'kb-doc' ? p('/knowledge') : p('/artifacts')}
                class="inline-flex max-w-48 items-center gap-2 truncate text-fg transition-colors hover:text-accent"
              >
                {#if a.refType === 'kb-doc'}<BookOpen size={14} class="shrink-0 text-muted" />{:else}<Gem size={14} class="shrink-0 text-muted" />{/if}
                <span class="truncate">{a.filename}</span>
              </a>
            {:else}
              <a href={attachmentUrl(a.id)} target="_blank" rel="noreferrer" class="inline-flex max-w-48 items-center gap-2 truncate text-fg transition-colors hover:text-accent">
                {#if isImage(a.mime)}
                  <img src={attachmentUrl(a.id)} alt="" class="h-5 w-5 rounded object-cover" />
                {:else}
                  <FileText size={14} class="shrink-0 text-muted" />
                {/if}
                <span class="truncate">{a.filename}</span>
                {#if a.size > 0}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{humanSize(a.size)}</span>{/if}
              </a>
            {/if}
            {#if canEdit}
              <button
                type="button"
                onclick={() => save(items.filter((x) => x.id !== a.id))}
                class="text-muted transition-colors hover:text-danger"
                title="Remove attachment"
              >
                <X size={13} />
              </button>
            {/if}
          </span>
        {/each}
      </div>
    {/if}
    {#if canEdit}<AttachButton disabled={false} onAttach={(a) => save([...items.filter((x) => x.id !== a.id), a])} />{/if}
  </Section>
{/if}
