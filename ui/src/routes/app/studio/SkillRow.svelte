<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { confirm, prompt } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { skillTitle } from '@/components/skills/skill-title'
  import { SKILLS_KEY, deleteSkill, renameSkill, type SkillOwner } from '@/lib/skills'
  import { copySkillTo, moveSkillTo } from '@/lib/workflows'
  import { navigateHref } from '@/router'

  let {
    owner,
    skill,
    owners,
    sharedBadge,
    canEdit,
    href,
  }: {
    owner: string
    skill: { name: string; description: string; platform?: boolean }
    owners: SkillOwner[]
    sharedBadge?: boolean
    canEdit: boolean
    /** Where the skill opens. */
    href: string
  } = $props()

  const qc = useQueryClient()
  const refresh = () => qc.invalidateQueries({ queryKey: SKILLS_KEY })
  const editableTargets = $derived(owners.filter((o) => o.canEdit && o.owner !== owner))
  const title = $derived(skillTitle(skill.name))
  const fail = (e: unknown) => void confirm({ title: 'That didn’t work', message: (e as Error).message, confirmLabel: 'OK' })

  const doRename = async () => {
    const to = await prompt({ title: 'Rename skill', message: 'Lowercase, with dashes for spaces. Agents load it by this name.', confirmLabel: 'Rename' })
    const name = to?.trim().toLowerCase().replace(/\s+/g, '-')
    if (!name || name === skill.name) return
    await renameSkill(owner, skill.name, name).then(refresh).catch(fail)
  }
  const doDelete = async () => {
    if (!(await confirm({ title: 'Delete skill', message: `Delete "${skill.name}"? Workflows bound to it will flag it as missing.`, confirmLabel: 'Delete', danger: true }))) return
    await deleteSkill(owner, skill.name).then(refresh).catch(fail)
  }
  // A skill this person cannot edit has an empty menu and keeps its cell, so the column holds down the list.
  const menu = $derived.by((): ContextMenuEntry[] => {
    if (!canEdit) return []
    return [
      { label: 'Edit', onSelect: () => void navigateHref(href) },
      { label: 'Rename', onSelect: () => void doRename() },
      ...(editableTargets.length
        ? [
            {
              label: 'Copy to',
              children: editableTargets.map((o) => ({
                label: o.owner === 'shared' ? 'Every agent' : o.label,
                onSelect: () => void copySkillTo(owner, skill.name, o.owner).then(refresh).catch(fail),
              })),
            },
          ]
        : []),
      ...(owner !== 'shared' && editableTargets.some((o) => o.owner === 'shared')
        ? [{ label: 'Move to every agent', onSelect: () => void moveSkillTo(owner, skill.name, 'shared').then(refresh).catch(fail) }]
        : []),
      'sep',
      { label: 'Delete skill', danger: true, onSelect: () => void doDelete() },
    ]
  })
</script>

<!-- The tooltip is the name agents load the skill by, which the row no longer shows. -->
<SettingsRow label={title} hint={skill.description || 'No description yet.'} openHref={href} openTitle={skill.name}>
  {#snippet meta()}
    {#if sharedBadge}<Chip class="-my-px">Shared</Chip>{/if}
    {#if skill.platform}<Chip class="-my-px">Built-in</Chip>{/if}
  {/snippet}
  <SettingsRowMenu label={`Actions for ${title}`} items={menu} />
</SettingsRow>
