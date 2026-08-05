<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getList } from '@/lib/fetch-json'
  import { listStagger } from '@/lib/motion'
  import SkillEditorModal from './SkillEditorModal.svelte'

  interface SkillSummary {
    name: string
    description: string
    files: string[]
  }

  let { slug, isAdmin }: { slug: string; isAdmin: boolean } = $props()

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['skills'],
    queryFn: (): Promise<Array<{ owner: string; skills: SkillSummary[] }>> =>
      getList<{ owner: string; skills: SkillSummary[] }>('/api/skills', 'owners'),
  }))
  // `owners` not carrying THIS agent is a real empty ("no skills yet"); a
  // failure is not, and must not be flattened into the same `?? []`.
  const skillsOf = (owners: Array<{ owner: string; skills: SkillSummary[] }>) =>
    owners.find((o) => o.owner === slug)?.skills ?? []
  let open = $state<string | null>(null)
  let newName = $state('')

  const create = async () => {
    const name = newName.trim().toLowerCase().replace(/\s+/g, '-')
    if (!name) return
    await fetch(`/api/skills/${slug}/${name}`, {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ content: `# ${name}\n\ndescription: what this skill is for\n\n## Steps\n\n1. \n` }),
    })
    newName = ''
    await qc.invalidateQueries({ queryKey: ['skills'] })
    open = name
  }
</script>

<div class="space-y-3">
  <QueryState
    {query}
    errorTitle="Could not load skills"
    errorVariant="compact"
    isEmpty={(owners) => skillsOf(owners).length === 0}
  >
    {#snippet skeleton()}<SkeletonRows rows={3} class="py-2" />{/snippet}
    {#snippet empty()}<EmptyState icon="✦" title="No skills yet" hint={isAdmin ? 'Add one below.' : undefined} />{/snippet}
    {#snippet children(owners)}
      <div class="divide-y divide-line" use:listStagger>
        {#each skillsOf(owners) as s (s.name)}
          <button type="button" onclick={() => (open = s.name)} class="flex w-full items-baseline gap-3 py-2.5 text-left transition-colors hover:bg-hover">
            <span class="shrink-0 font-sans text-sm font-medium text-fg">{s.name}</span>
            <span class="min-w-0 truncate font-sans text-sm text-muted">{s.description}</span>
            {#if s.files.length > 1}<span class="ml-auto shrink-0 font-mono text-[11px] text-muted">{s.files.length} files</span>{/if}
          </button>
        {/each}
      </div>
    {/snippet}
  </QueryState>
  {#if isAdmin}
    <div class="flex items-center gap-2 pt-1">
      <Input size="sm" bind:value={newName} placeholder="new-skill-name" class="w-52" onkeydown={(e) => e.key === 'Enter' && void create()} />
      <Button size="sm" onclick={() => void create()} disabled={!newName.trim()}>
        Add skill
      </Button>
    </div>
  {/if}
  {#if open}<SkillEditorModal {slug} name={open} {isAdmin} onClose={() => (open = null)} />{/if}
</div>
