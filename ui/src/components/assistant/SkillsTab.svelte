<script lang="ts" module>
  export interface SkillSummary {
    name: string
    description: string
  }

  const SKILL_TEMPLATE = (name: string) => `# ${name}

Describe when your assistant should use this skill and how.

## Steps
1. 
`
</script>

<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import { Plus } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkillEditor from './SkillEditor.svelte'
  import { getList } from '@/lib/fetch-json'
  import { type Assistant } from '@/lib/assistant'
  import { fade, listStagger, slide, QUICK } from '@/lib/motion'

  let { assistant }: { assistant: Assistant } = $props()

  const qc = useQueryClient()
  // `owners` with no entry for this assistant IS a real empty; a 5xx is not.
  // Narrowing happens after the read so the failure can still reject.
  const query = createQuery(() => ({
    queryKey: ['assistant-skills', assistant.slug],
    queryFn: async (): Promise<SkillSummary[]> => {
      const owners = await getList<{ owner: string; skills: SkillSummary[] }>('/api/skills', 'owners')
      return owners.find((o) => o.owner === assistant.slug)?.skills ?? []
    },
  }))
  let open = $state<string | null>(null)
  let newName = $state('')
  let busy = $state(false)
  let error = $state<string | null>(null)

  const refresh = async () => qc.invalidateQueries({ queryKey: ['assistant-skills', assistant.slug] })

  const save = async (name: string, body: string) => {
    busy = true
    error = null
    try {
      const r = await fetch(`/api/skills/${assistant.slug}/${name}`, {
        method: 'PUT',
        credentials: 'same-origin',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ content: body }),
      })
      const j = (await r.json().catch(() => ({}))) as { error?: string }
      if (!r.ok || j.error) {
        error = j.error ?? 'could not save'
        return
      }
      await refresh()
    } finally {
      busy = false
    }
  }

  const add = () => {
    const n = newName
    void save(n, SKILL_TEMPLATE(n)).then(() => {
      open = n
      newName = ''
    })
  }
</script>

<div class="space-y-4">
  <SectionHeader
    class="mb-0"
    title="Skills"
    info="Step-by-step playbooks your assistant follows for recurring jobs: weekly summaries, travel planning, whatever you teach it."
    action={query.data && query.data.length > 0 ? String(query.data.length).padStart(2, '0') : undefined}
  />
  <!-- The count is meta ABOUT the read, so it only exists once the read
       resolved: `rows.length` on a failure would print `00`, which is a
       claim ("you have no skills") the server never made. -->
  <QueryState {query} errorTitle="Could not load your assistant's skills" errorVariant="compact">
    {#snippet skeleton()}
      <ul aria-hidden="true" class="divide-y divide-line rounded-lg border border-line">
        {#each [0, 1, 2] as i (i)}
          <li class="flex items-center gap-2 px-3 py-3">
            <Skeleton class="h-3 w-28 rounded-full" delay={i * 0.12} />
            <Skeleton class="h-2.5 w-44 rounded-full" delay={i * 0.12 + 0.12} />
          </li>
        {/each}
      </ul>
    {/snippet}
    {#snippet empty()}
      <EmptyState icon="✦" title="No skills yet" hint="Teach it its first playbook below." />
    {/snippet}
    {#snippet children(skills)}
      <ul class="divide-y divide-line rounded-lg border border-line" use:listStagger>
        {#each skills as s (s.name)}
          <li in:fade={{ duration: 150 }} out:fade={QUICK}>
            <button
              type="button"
              onclick={() => (open = s.name)}
              class="flex w-full items-baseline gap-2 px-3 py-2 text-left transition-colors hover:bg-hover"
            >
              <span class="text-sm text-fg">{s.name}</span>
              <span class="min-w-0 flex-1 truncate text-xs text-muted">{s.description}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/snippet}
  </QueryState>
  <div class="flex items-center gap-2">
    <Input
      size="sm"
      bind:value={newName}
      oninput={() => (newName = newName.toLowerCase().replace(/[^a-z0-9._-]/g, '-'))}
      placeholder="new-skill-name"
    />
    <Button
      size="sm"
      variant="outline"
      disabled={busy || !/^[a-z0-9][a-z0-9._-]*$/.test(newName)}
      onclick={add}
    >
      <Plus size={14} /> Add
    </Button>
  </div>
  {#if error}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</p>{/if}
  {#if open !== null}
    <SkillEditor {assistant} name={open} onClose={() => (open = null)} onChanged={() => void refresh()} />
  {/if}
</div>
