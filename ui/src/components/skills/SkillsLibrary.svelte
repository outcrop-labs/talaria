<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Button from '@/components/ui/Button.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import {
    SKILLS_KEY,
    SKILL_TEMPLATE,
    saveSkill,
    skillName,
    skillsOf,
    useSkill,
    useSkills,
    type SkillSummary,
  } from '@/lib/skills'
  import SkillEditor from './SkillEditor.svelte'

  // ONE OWNER'S SKILLS, as a library: pick on the left, read it on the right,
  // Edit opens the full workspace editor. Same shape as Templates and the
  // agent role library — a skill IS a record you pick and edit, so it gets the
  // same surface rather than a third list-plus-modal of its own.
  //
  // It replaces two list views (the assistant's Skills tab and the fleet
  // agent's) that had the same job and differed in row shape, create control,
  // create template, name sanitising and which editor they opened.
  let {
    owner,
    ownerLabel,
    canEdit = true,
    surface = 'panel',
    class: className,
  }: {
    owner: string
    ownerLabel: string
    canEdit?: boolean
    /** `well` when this sits inside a modal or a section — see LibraryPane. */
    surface?: 'panel' | 'well' | 'bare'
    class?: string
  } = $props()

  const qc = useQueryClient()
  const query = useSkills()
  // A getter, not a template literal: read once, the title would freeze on the
  // owner this pane first mounted for.
  const list = listQuery(query, {
    get title() {
      return `Could not load ${ownerLabel}'s skills`
    },
  })
  // An owner missing from a SUCCESSFUL read is a real empty; a failed read is
  // not, and `listQuery` has already kept those apart before we narrow.
  const skills = $derived(skillsOf(list.rows, owner))

  let selected = $state<string | null>(null)
  let editing = $state(false)
  let createError = $state<unknown>(null)

  // The selected skill's body, for the read pane. Reuses the same key the
  // editor reads, so opening the editor is a cache hit rather than a refetch.
  const doc = useSkill(() => owner, () => selected)

  const create = async (raw: string) => {
    const name = skillName(raw)
    if (!name) return
    createError = null
    try {
      // A skill exists once its SKILL.md does, so creation is the first save.
      await saveSkill(owner, name, SKILL_TEMPLATE(name))
      await qc.invalidateQueries({ queryKey: SKILLS_KEY })
      selected = name
      editing = true
    } catch (e) {
      // This used to be `await fetch(...)` with the status ignored, so a
      // refused create silently opened an editor onto a skill that was never
      // written.
      createError = e
    }
  }
</script>

<LibraryPane
  groups={[{ items: skills }]}
  idOf={(s: SkillSummary) => s.name}
  labelOf={(s: SkillSummary) => s.name}
  selectedId={selected}
  onSelect={(s: SkillSummary) => (selected = s.name)}
  pending={list.pending}
  notice={list.notice}
  onCreate={canEdit ? create : undefined}
  createLabel="New skill"
  createPlaceholder="new-skill-name"
  {surface}
  class={className}
>
  {#snippet row(s: SkillSummary)}
    <!-- Name over description: the name is what you scan for, and the
         description is what tells you it is the right one. -->
    <span class="block truncate text-fg">{s.name}</span>
    <span class="block truncate text-[11px] text-muted">{s.description}</span>
  {/snippet}

  {#snippet empty()}
    <EmptyState
      icon="✦"
      title="No skills yet"
      hint={canEdit
        ? 'A skill is a playbook for a recurring job — name one to teach it.'
        : `${ownerLabel} has not been taught anything yet.`}
    />
  {/snippet}

  {#snippet detail()}
    {#if selected}
      <div class="flex min-h-0 flex-1 flex-col">
        <div class="flex h-11 shrink-0 items-center gap-2 border-b border-line-subtle px-4">
          <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{selected}</span>
          <Button size="sm" variant="outline" onclick={() => (editing = true)}>
            {canEdit ? 'Edit' : 'Open'}
          </Button>
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-6">
          {#if doc.isError}
            <QueryError
              variant="compact"
              error={doc.error}
              title={`Could not open ${selected}`}
              onRetry={() => void doc.refetch()}
            />
          {:else if !doc.data}
            <SkeletonRows rows={6} />
          {:else}
            <Markdown class="tiptap" children={doc.data.content} />
          {/if}
        </div>
      </div>
    {/if}
  {/snippet}
</LibraryPane>

{#if createError}
  <QueryError variant="inline" error={createError} title="Could not create that skill" />
{/if}

{#if editing && selected}
  <SkillEditor
    {owner}
    {ownerLabel}
    name={selected}
    {canEdit}
    onClose={() => (editing = false)}
  />
{/if}
