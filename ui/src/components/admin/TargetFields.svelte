<script lang="ts">
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import Input from '@/components/ui/Input.svelte'
  import ToggleField from './ToggleField.svelte'
  import { secretHint } from './secret-hint'
  import type { TargetConfig } from './storage'

  // Bucket credential fields, shared by the external target and the replica.
  // `id` keeps the two sets' labels apart when both are on one page.
  let {
    id,
    t,
    secret,
    onChange,
    onSecret,
  }: {
    id: string
    t: TargetConfig
    secret: string
    onChange: (patch: Partial<TargetConfig>) => void
    onSecret: (v: string) => void
  } = $props()
</script>

<div class="grid gap-x-4 gap-y-5 sm:grid-cols-2">
  <SettingsField label="Endpoint" for="{id}-endpoint" class="sm:col-span-2">
    <Input id="{id}-endpoint" size="sm" class="font-mono" value={t.endpoint} oninput={(e) => onChange({ endpoint: e.currentTarget.value })} placeholder="https://s3.us-west-004.backblazeb2.com" />
  </SettingsField>
  <SettingsField label="Bucket" for="{id}-bucket">
    <Input id="{id}-bucket" size="sm" class="font-mono" value={t.bucket} oninput={(e) => onChange({ bucket: e.currentTarget.value })} placeholder="talaria-uploads" />
  </SettingsField>
  <SettingsField label="Region" for="{id}-region" hint="Blank reads it from the endpoint.">
    <Input id="{id}-region" size="sm" class="font-mono" value={t.region} oninput={(e) => onChange({ region: e.currentTarget.value })} placeholder="auto" />
  </SettingsField>
  <SettingsField label="Access key ID" for="{id}-access-key">
    <Input id="{id}-access-key" size="sm" class="font-mono" value={t.accessKeyId} oninput={(e) => onChange({ accessKeyId: e.currentTarget.value })} />
  </SettingsField>
  <SettingsField label="Secret access key" for="{id}-secret" hint={secretHint(t.hasSecret)}>
    <Input id="{id}-secret" size="sm" type="password" autocomplete="off" value={secret} oninput={(e) => onSecret(e.currentTarget.value)} />
  </SettingsField>
  <SettingsField label="Key prefix" for="{id}-prefix" hint="Optional, and ends with a slash." class="sm:col-span-2">
    <Input id="{id}-prefix" size="sm" class="font-mono" value={t.prefix} oninput={(e) => onChange({ prefix: e.currentTarget.value })} placeholder="talaria/" />
  </SettingsField>
  <div class="sm:col-span-2">
    <ToggleField
      label="Path-style requests"
      hint="Works everywhere, so turn it off only for virtual-host buckets."
      checked={t.pathStyle}
      onChange={(checked) => onChange({ pathStyle: checked })}
    />
  </div>
</div>
