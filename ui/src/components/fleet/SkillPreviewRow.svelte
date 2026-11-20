<script lang="ts">
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import { slide } from '@/lib/motion'

  let { skill, onRemove }: { skill: { name: string; content: string }; onRemove: () => void } = $props()

  let expanded = $state(false)
</script>

<li class="px-4 py-2.5">
  <div class="flex items-center gap-4">
    <button type="button" aria-expanded={expanded} onclick={() => (expanded = !expanded)} class="min-w-0 flex-1 truncate text-left font-sans text-sm leading-5 text-fg">
      {skill.name}
      <span class="ml-2 text-[13px] text-muted">{expanded ? 'Hide' : 'View'}</span>
    </button>
    <DangerLink onClick={onRemove}>Remove</DangerLink>
  </div>
  {#if expanded}
    <pre
      transition:slide={{ duration: 150 }} class="mt-2 max-h-48 overflow-y-auto whitespace-pre-wrap font-mono text-xs leading-5 text-muted">{skill.content}</pre>
  {/if}
</li>
