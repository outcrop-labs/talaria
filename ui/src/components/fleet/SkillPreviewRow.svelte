<script lang="ts">
  import { Trash2 } from '@lucide/svelte'
  import { slide } from '@/lib/motion'

  let { skill, onRemove }: { skill: { name: string; content: string }; onRemove: () => void } = $props()

  let expanded = $state(false)
</script>

<li class="px-3.5 py-2.5">
  <div class="flex items-center gap-2">
    <button type="button" onclick={() => (expanded = !expanded)} class="min-w-0 flex-1 text-left">
      <span class="text-sm text-fg">{skill.name}</span>
      <span class="ml-2 text-xs text-muted">{expanded ? 'hide' : 'view'}</span>
    </button>
    <button type="button" title="Drop this skill" onclick={onRemove} class="shrink-0 text-muted transition-colors hover:text-danger">
      <Trash2 size={14} />
    </button>
  </div>
  {#if expanded}
    <pre
      transition:slide={{ duration: 150 }} class="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap rounded-md border border-line bg-surface p-3 font-mono text-xs leading-5 text-muted">{skill.content}</pre>
  {/if}
</li>
