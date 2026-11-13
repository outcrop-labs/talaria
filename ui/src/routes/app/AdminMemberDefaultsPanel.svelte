<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'
  import { permGroups, type PermsData } from './admin'

  /** Org-wide member defaults — what a plain member can do before any per-user
   *  override. Each is a switch; returning to the shipped default clears the row. */
  let { perms }: { perms: PermsData } = $props()

  const qc = useQueryClient()
  const set = async (perm: string, enabled: boolean | null) => {
    try {
      await putJson<{ ok: true }>('/api/admin/permissions', { orgDefault: { perm, enabled } })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['admin-permissions'] })
  }
</script>

<SettingsSection
  title="Member permissions"
  description="What every member can do unless you change it for one person."
  info="Admins can always do all of these. To make an exception for one person, use Edit on their row in People."
>
  <div class="space-y-5">
    {#each permGroups(perms.catalog) as [group, entries] (group)}
      <div>
        <h3 class="mb-2 font-sans text-xs font-medium leading-4 text-muted">{group}</h3>
        <SettingsGroup>
          {#each entries as p (p.id)}
            {@const effective = perms.orgDefaults[p.id] ?? p.memberDefault}
            <SettingsRow label={p.label} info={p.hint}>
              {#snippet meta()}
                {#if perms.orgDefaults[p.id] !== undefined && perms.orgDefaults[p.id] !== p.memberDefault}
                  <Chip tone="warn" title="Changed from Talaria's own default">Changed</Chip>
                {/if}
              {/snippet}
              <Toggle
                title={p.label}
                checked={effective}
                onChange={() => void set(p.id, !effective === p.memberDefault ? null : !effective)}
              />
            </SettingsRow>
          {/each}
        </SettingsGroup>
      </div>
    {/each}
  </div>
</SettingsSection>
