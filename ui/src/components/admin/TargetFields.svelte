<script lang="ts">
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import type { TargetConfig } from './storage'

  // Bucket credential fields, shared by the external target and the replica.
  let {
    t,
    secret,
    onChange,
    onSecret,
  }: { t: TargetConfig; secret: string; onChange: (patch: Partial<TargetConfig>) => void; onSecret: (v: string) => void } = $props()
</script>

<label class="text-xs text-muted">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Endpoint</span>
  <Input value={t.endpoint} oninput={(e) => onChange({ endpoint: e.currentTarget.value })} placeholder="https://s3.us-west-004.backblazeb2.com" class="mt-1 w-full" />
</label>
<label class="text-xs text-muted">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Bucket</span>
  <Input value={t.bucket} oninput={(e) => onChange({ bucket: e.currentTarget.value })} placeholder="talaria-uploads" class="mt-1 w-full" />
</label>
<label class="text-xs text-muted">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Region</span> <span class="opacity-70">(blank = derived from endpoint)</span>
  <Input value={t.region} oninput={(e) => onChange({ region: e.currentTarget.value })} placeholder="auto" class="mt-1 w-full" />
</label>
<label class="text-xs text-muted">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Key prefix</span> <span class="opacity-70">(optional, ends with /)</span>
  <Input value={t.prefix} oninput={(e) => onChange({ prefix: e.currentTarget.value })} placeholder="talaria/" class="mt-1 w-full" />
</label>
<label class="text-xs text-muted">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Access key ID</span>
  <Input value={t.accessKeyId} oninput={(e) => onChange({ accessKeyId: e.currentTarget.value })} class="mt-1 w-full" />
</label>
<label class="text-xs text-muted">
  <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Secret access key</span>
  <Input type="password" value={secret} oninput={(e) => onSecret(e.currentTarget.value)} placeholder={t.hasSecret ? '•••••••• (saved)' : ''} class="mt-1 w-full" />
</label>
<Checkbox checked={t.pathStyle} onChange={(checked) => onChange({ pathStyle: checked })} class="gap-2 sm:col-span-2">
  {#snippet label()}
    Path-style requests <span class="opacity-70">(works everywhere; uncheck only for virtual-host buckets)</span>
  {/snippet}
</Checkbox>
