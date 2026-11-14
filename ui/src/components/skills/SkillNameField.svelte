<script lang="ts">
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { useUiVoice } from '@/lib/ui-voice'

  // The skill's directory name, as a field of its record. The record's Save
  // carries the rename.
  let { value = $bindable() }: { value: string } = $props()

  const sans = useUiVoice() === 'sans'
  const id = $props.id()
  const info = "The skill's directory name: lowercase letters, digits, dots, underscores and hyphens. Renaming is part of the record's Save."
</script>

{#if sans}
  <SettingsField class="mb-4 shrink-0" label="Skill name" for={id} hint="Agents load the skill by this name." {info}>
    <Input {id} size="sm" bind:value class="max-w-sm font-mono" />
  </SettingsField>
{:else}
  <div class="mb-4 shrink-0 rounded-lg border border-line bg-card/40 p-3">
    <div class="mb-1.5 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Skill name</span><!-- voice-ok: mono branch, for the agent dialog opened from chat -->
      <InfoTip text={info} />
    </div>
    <Input size="sm" bind:value class="max-w-sm font-mono" />
  </div>
{/if}
