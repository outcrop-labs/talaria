<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { inlineEditKeys } from '@/components/ui/control'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { putJson } from '@/lib/fetch-json'
  import { toastError } from '@/lib/toast.svelte'

  // Where the org account's agents build: a Shared Drive so files are team-owned,
  // a specific calendar, and an optional send-as alias for outgoing mail. Rows
  // of the shared Google account card.
  let { targets }: { targets: { driveFolderId: string | null; calendarId: string | null; sendAs: string | null } } = $props()

  const qc = useQueryClient()
  // Seed-once form drafts: the parent gates this section behind the account's
  // loaded config, so re-seeding on every target refresh would clobber
  // in-progress edits; `dirty` compares against the live prop instead.
  // svelte-ignore state_referenced_locally -- reason: seed-once form drafts; parent gates on loaded data, dirty tracks the live prop
  let drive = $state(targets.driveFolderId ?? '')
  // svelte-ignore state_referenced_locally -- reason: seed-once form drafts; parent gates on loaded data, dirty tracks the live prop
  let cal = $state(targets.calendarId ?? '')
  // svelte-ignore state_referenced_locally -- reason: seed-once form drafts; parent gates on loaded data, dirty tracks the live prop
  let sendAs = $state(targets.sendAs ?? '')
  const savedFlash = useSavedFlash()
  const dirty = $derived(drive !== (targets.driveFolderId ?? '') || cal !== (targets.calendarId ?? '') || sendAs !== (targets.sendAs ?? ''))
  // Which row says Saved: the one whose field was just left.
  let savedField = $state<'drive' | 'cal' | 'sendAs' | null>(null)

  const save = async (field: 'drive' | 'cal' | 'sendAs') => {
    if (!dirty) return
    try {
      await putJson<{ ok: true }>('/api/integrations/google/org', { driveFolderId: drive, calendarId: cal, sendAs })
    } catch (e) {
      toastError('Save failed', e)
      return
    }
    await qc.invalidateQueries({ queryKey: ['org-google'] })
    savedField = field
    savedFlash.flash()
  }
</script>

{#snippet saved(field: 'drive' | 'cal' | 'sendAs')}
  {#if savedFlash.saved && savedField === field}<span class="font-sans text-[13px] leading-5 text-success">Saved</span>{/if}
{/snippet}

<SettingsRow
  label="Save files to"
  hint="A shared drive or folder ID. Blank uses the account's My Drive."
  info="Files agents create are saved here, so the team owns them. The shared account must be a member of the shared drive."
  as="label"
>
  {@render saved('drive')}
  <Input
    size="sm"
    class="w-[280px] font-mono"
    bind:value={drive}
    onblur={() => void save('drive')}
    onkeydown={inlineEditKeys(() => (drive = targets.driveFolderId ?? ''))}
    placeholder="Shared drive or folder ID"
  />
</SettingsRow>
<SettingsRow label="Add events to" hint="A calendar ID. Blank uses the account's main calendar." as="label">
  {@render saved('cal')}
  <Input
    size="sm"
    class="w-[280px] font-mono"
    bind:value={cal}
    onblur={() => void save('cal')}
    onkeydown={inlineEditKeys(() => (cal = targets.calendarId ?? ''))}
    placeholder="team@group.calendar.google.com"
  />
</SettingsRow>
<SettingsRow label="Send mail as" hint="A verified alias on the account. Blank uses its own address." as="label">
  {@render saved('sendAs')}
  <Input
    size="sm"
    class="w-[280px]"
    bind:value={sendAs}
    onblur={() => void save('sendAs')}
    onkeydown={inlineEditKeys(() => (sendAs = targets.sendAs ?? ''))}
    placeholder="support@yourdomain.com"
  />
</SettingsRow>
