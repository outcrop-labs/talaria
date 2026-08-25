<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import LibraryPane from '@/components/ui/LibraryPane.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import RecordEditor from '@/components/editor/RecordEditor.svelte'
  import { confirmDelete } from '@/components/ui/confirm.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import {
    SKILLS_KEY,
    SKILL_TEMPLATE,
    deleteSkill,
    renameSkill,
    saveSkill,
    skillKey,
    skillName,
    skillsOf,
    useSkill,
    useSkills,
    type SkillSummary,
  } from '@/lib/skills'
  import { draftSkillForm } from '@/lib/muse.svelte'

  // ONE OWNER'S SKILLS, as a library: pick on the left, and the record's
  // surface — its name, its SKILL.md workbench, its whole-form Muse — is the
  // detail on the right. The editor is not opened; it IS the view.
  //
  // It replaces two list views (the assistant's Skills tab and the fleet
  // agent's) that had the same job and differed in row shape, create control,
  // create template, name sanitising and which editor they opened.
  let {
    owner,
    ownerLabel,
    canEdit = true,
    surface = 'panel',
    onChanged,
    class: className,
  }: {
    owner: string
    ownerLabel: string
    canEdit?: boolean
    /** `well` when this sits inside a modal or a section — see LibraryPane. */
    surface?: 'panel' | 'well' | 'bare'
    /** Extra invalidation for a caller with its own list key (the Studio). */
    onChanged?: () => void
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
  let createError = $state<unknown>(null)

  // The selected skill's body, for the workbench. Reuses the same key the
  // editor reads, so picking a skill is a cache hit rather than a refetch.
  const doc = useSkill(() => owner, () => selected)

  // The recorded name the record holds, and the name being typed. A skill name
  // is the skill's DIRECTORY: lowercase, the write path's alphabet — the input
  // is free text, the SAVE coerces it, and the dirty flag compares coerced to
  // saved so a no-op recase does not count as unsaved work.
  let name = $state('')
  $effect(() => {
    if (selected && (doc.data || doc.isError)) name = selected
  })

  const create = async (raw: string) => {
    const n = skillName(raw)
    if (!n) return
    createError = null
    try {
      // A skill exists once its SKILL.md does, so creation is the first save.
      await saveSkill(owner, n, SKILL_TEMPLATE(n))
      await qc.invalidateQueries({ queryKey: SKILLS_KEY })
      selected = n
    } catch (e) {
      // This used to be `await fetch(...)` with the status ignored, so a
      // refused create silently opened an editor onto a skill that was never
      // written.
      createError = e
    }
  }

  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: SKILLS_KEY })
    await qc.invalidateQueries({ queryKey: skillKey(owner, selected ?? '') })
    onChanged?.()
  }

  // The record's Save. A save that renames is TWO writes, in this order: move
  // the directory first, then write the content under the new name — so a
  // failed content write after a rename leaves a skill that exists under its
  // new name with its old content, which is a state the library can show.
  const saveAll = async (content: string) => {
    if (!selected) return
    const toName = skillName(name) || selected
    if (toName !== selected) {
      await renameSkill(owner, selected, toName)
      selected = toName
    }
    await saveSkill(owner, toName, content)
    await refresh()
  }

  const remove = async () => {
    if (!selected) return
    if (
      !(await confirmDelete({
        what: 'skill',
        name: selected,
        detail: `Deleting “${selected}” removes its whole directory. Anything bound to it will flag it as missing.`,
      }))
    )
      return
    try {
      await deleteSkill(owner, selected)
      await refresh()
      selected = null
    } catch (e) {
      createError = e
    }
  }

  const museContext = $derived(
    owner === 'shared'
      ? 'A shared skill every agent on the team can use. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.'
      : `A skill for the "${ownerLabel}" agent. SKILL.md format: a heading, a "When to use" line, then concrete numbered steps.`,
  )
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
        ? 'A skill is a playbook for a recurring job. Name one to teach it.'
        : `${ownerLabel} has not been taught anything yet.`}
    />
  {/snippet}

  {#snippet detail()}
    {#if selected}
      <!-- No padding: the record surface owns its inset, so its pinned menu
           sits flush at the pane's edges. -->
      <div class="min-h-0 flex-1 overflow-hidden">
        {#if doc.isError}
          <QueryError variant="compact" error={doc.error} title={`Could not open ${selected}`} onRetry={() => void doc.refetch()} />
        {:else if !doc.data}
          <SkeletonRows rows={6} />
        {:else}
          <!-- The record's surface, keyed on the record: picking another skill
               reseeds the name input and the workbench; nothing about the
               previous selection leaks into the next. -->
          {#key selected}
            <RecordEditor
              kind="skill"
              title={selected}
              meta={ownerLabel}
              subtitle={canEdit ? 'Read live; agents pick up edits on their next run.' : 'Read-only here; this record is maintained elsewhere.'}
              fieldsDirty={canEdit ? skillName(name) !== selected : false}
              onDelete={canEdit ? () => void remove() : undefined}
              doc={{
                value: doc.data.content,
                editable: canEdit,
                // Read-only records never reach a save; the no-op keeps the
                // surface's contract whole.
                onSave: canEdit ? saveAll : () => Promise.resolve(),
                history: { kind: 'skill', owner, name: selected },
                // No doc.muse: the whole-form Muse drafts name AND content,
                // so the workbench carries no composer of its own.
              }}
              // The closure may outlive its {#if selected} scope, so the
              // narrow does not reach in: guard it.
              onCancel={canEdit ? () => {
                if (selected) name = selected
              } : undefined}
              formMuse={
                canEdit
                  ? {
                      label: 'skill',
                      current: (docText) => ({ name: skillName(name) || selected, content: docText }),
                      draft: async (input, signal) => draftSkillForm({ ...input, context: museContext }, signal),
                      fields: (d: { name?: string; content?: string; error?: string }) => [
                        { label: 'name', value: String(d.name ?? '').slice(0, 40) },
                      ],
                      docOf: (d: { content?: string }) => String(d.content ?? ''),
                      apply: (d: { name?: string; content?: string }) => {
                        const n = String(d.name ?? '').trim()
                        if (n) name = n
                      },
                    }
                  : undefined
              }
              class="h-full"
            >
              {#snippet fields(_)}
                {#if canEdit}
                  <div class="mb-4 shrink-0 rounded-lg border border-line bg-card/40 p-3">
                    <div class="mb-1.5 flex items-center gap-1.5">
                      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Skill name</span>
                      <InfoTip text="The skill's directory name: lowercase letters, digits, dots, underscores and hyphens. Renaming is part of the record's Save." />
                    </div>
                    <Input size="sm" bind:value={name} class="max-w-sm font-mono" />
                  </div>
                {/if}
              {/snippet}
            </RecordEditor>
          {/key}
        {/if}
      </div>
    {/if}
  {/snippet}
</LibraryPane>

{#if createError}
  <QueryError variant="inline" error={createError} title="Could not save this skill" />
{/if}
