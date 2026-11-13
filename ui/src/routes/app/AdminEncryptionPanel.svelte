<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import { errorMessage, getJson, postJson } from '@/lib/fetch-json'

  type EncryptionData = {
    keyVersion: number | null
    rotatedAt: string | null
    secretCount: number
    rootSource: string
    algorithm: string
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['admin-encryption'],
    queryFn: (): Promise<EncryptionData> => getJson<EncryptionData>('/api/admin/encryption'),
  }))
  let busy = $state(false)
  let msg = $state<string | null>(null)
  let failed = $state(false)
  let newRoot = $state('')

  const rotate = async () => {
    if (!(await confirm({ title: 'Rotate encryption key', message: 'Rotate the encryption key? Every stored secret is re-encrypted under a fresh key in one pass. Existing secrets keep working.', confirmLabel: 'Rotate' }))) return
    busy = true
    msg = null
    failed = false
    try {
      const j = await postJson<{ ok: true; version: number; reencrypted: number; rootRewrapped: boolean }>(
        '/api/admin/encryption',
        newRoot.trim() ? { newRootSecret: newRoot.trim() } : {},
      )
      msg = `Re-encrypted ${j.reencrypted} secret${j.reencrypted === 1 ? '' : 's'}. The key is now version ${j.version}.`
      newRoot = ''
      await qc.invalidateQueries({ queryKey: ['admin-encryption'] })
    } catch (e) {
      msg = errorMessage(e)
      failed = true
    } finally {
      busy = false
    }
  }
</script>

<SettingsSection
  title="Encryption"
  description="Every stored secret is encrypted before it is saved."
  info="A random data key encrypts the secrets (AES-256-GCM). That key is itself locked by the root key, which lives where the server is configured and never in the database."
>
  <SettingsGroup>
    <!-- "Secrets protected: —" over a failed read looks like a fact about the
         key store rather than a fact about the request, so QueryState's error
         branch owns that state. -->
    <QueryState query={query} errorTitle="Could not load encryption status" errorVariant="compact">
      {#snippet skeleton()}
        <div class="space-y-3 px-5 py-4">
          {#each Array.from({ length: 4 }, (_, i) => i) as i (i)}
            <Skeleton class="h-3 w-56 rounded-full" />
          {/each}
        </div>
      {/snippet}
      {#snippet children(data)}
        <dl class="grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4 font-sans text-[13px] leading-5">
          <dt class="text-muted">Key version</dt>
          <dd class="tabular-nums text-fg">{data.keyVersion ?? 'None yet'}</dd>
          <dt class="text-muted">Secrets protected</dt>
          <dd class="tabular-nums text-fg">{data.secretCount}</dd>
          <dt class="text-muted">Root key source</dt>
          <dd><code class="font-mono text-xs text-fg">{data.rootSource}</code></dd>
          <dt class="text-muted">Last rotated</dt>
          <dd class="text-fg">{data.rotatedAt ? new Date(data.rotatedAt).toLocaleString() : 'Never'}</dd>
        </dl>
      {/snippet}
    </QueryState>
    <SettingsRow
      label="Rotate the key"
      hint="Re-encrypts every secret under a new key and retires the old one for good."
      info="Leave the field empty to keep the current root key. If you enter a new one, set TALARIA_SECRET_KEY, or the key file, to the same value before the next restart."
    >
      <Input
        size="sm"
        type="password"
        aria-label="New root key, optional"
        bind:value={newRoot}
        placeholder="New root key (optional)"
        autocomplete="off"
        class="w-[280px]"
      />
      <Button size="md" variant="outline" onclick={() => void rotate()} disabled={busy}>
        {busy ? 'Rotating' : 'Rotate key'}
      </Button>
    </SettingsRow>
    {#if busy}
      <div class="px-5 py-4">
        <Generating site="admin/key-rotate" label="Re-encrypting provider keys, agent secrets and sign-in tokens" lines={2} />
      </div>
    {/if}
    {#if newRoot.trim()}
      <p class="px-5 py-3 font-sans text-[13px] leading-5 text-muted">
        After rotating, set <code class="font-mono text-xs text-fg">TALARIA_SECRET_KEY</code> or the key file to the new root key before the next restart.
      </p>
    {/if}
    {#if msg}
      <p class="px-5 py-3 font-sans text-[13px] leading-5 {failed ? 'text-danger' : 'text-success'}">{msg}</p>
    {/if}
  </SettingsGroup>
</SettingsSection>
