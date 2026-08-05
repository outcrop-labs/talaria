<script lang="ts">
  import { Plus, Users } from '@lucide/svelte'
  import { useHasPerm } from '@/lib/session'

  let { onNew, onTeams }: { onNew: () => void; onTeams: () => void } = $props()

  const mayCreate = useHasPerm('boards.create')
</script>

<!-- Divided off from the list so the actions don't read as another board row. -->
<div class="mt-1.5 flex flex-col gap-px border-t border-line-subtle pt-1.5">
  {#if mayCreate.current}
    <button onclick={onNew} class="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors duration-[120ms] hover:text-accent">
      <Plus size={13} class="shrink-0" /> New board
    </button>
  {/if}
  <button onclick={onTeams} class="flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-xs text-muted transition-colors duration-[120ms] hover:text-accent">
    <Users size={13} class="shrink-0" /> Manage teams
  </button>
</div>
