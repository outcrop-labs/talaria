<script lang="ts">
  import { Trash2 } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { relativeTime } from '@/lib/fleet'
  import type { KbComment } from './knowledge.svelte'

  let { c, meId, onDelete }: { c: KbComment; meId: string | null; onDelete: () => void } = $props()
</script>

<div class="group/comment">
  <div class="flex items-baseline gap-1.5">
    <Avatar name={c.author} class="h-4 w-4 self-center text-[8px]" />
    <span class="text-xs font-medium text-fg">{c.author}</span>
    <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{relativeTime(c.createdAt)}</span>
    {#if c.authorUserId === meId}
      <button
        type="button"
        onclick={onDelete}
        class="ml-auto text-muted opacity-0 transition-opacity hover:text-danger group-hover/comment:opacity-100"
        title="Delete your comment"
      >
        <Trash2 size={11} />
      </button>
    {/if}
  </div>
  <div class="whitespace-pre-wrap pl-5 font-sans text-xs leading-relaxed text-fg">{c.content}</div>
</div>
