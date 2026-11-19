<script lang="ts">
  import Toggle from '@/components/ui/Toggle.svelte'

  // A switch in a dialog: what it does on the left, the switch on the right,
  // the way a `SettingsRow` reads on a page.
  let {
    label,
    hint,
    checked,
    disabled = false,
    onChange,
  }: {
    label: string
    hint?: string
    checked: boolean
    disabled?: boolean
    onChange: (checked: boolean) => void
  } = $props()
</script>

<div class="flex items-center gap-6">
  <div class="min-w-0 flex-1">
    <div class="font-sans text-[13px] font-medium leading-5 text-fg">{label}</div>
    {#if hint}<div class="font-sans text-[13px] leading-5 text-muted">{hint}</div>{/if}
  </div>
  <Toggle title={label} {checked} {disabled} {onChange} />
</div>
