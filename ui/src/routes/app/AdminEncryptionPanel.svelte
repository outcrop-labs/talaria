<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Generating from '@/components/ui/Generating.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
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
  const data = $derived(query.data)
  let busy = $state(false)
  let msg = $state<string | null>(null)
  let newRoot = $state('')

  const rotate = async () => {
    if (!(await confirm({ title: 'Rotate encryption key', message: 'Rotate the encryption key? Every stored secret is re-encrypted under a fresh key in one pass. Existing secrets keep working.', confirmLabel: 'Rotate' }))) return
    busy = true
    msg = null
    try {
      const j = await postJson<{ ok: true; version: number; reencrypted: number; rootRewrapped: boolean }>(
        '/api/admin/encryption',
        newRoot.trim() ? { newRootSecret: newRoot.trim() } : {},
      )
      msg = `Re-encrypted ${j.reencrypted} secret${j.reencrypted === 1 ? '' : 's'} · now key v${j.version}`
      newRoot = ''
      await qc.invalidateQueries({ queryKey: ['admin-encryption'] })
    } catch (e) {
      msg = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<Panel>
  <SectionHeader
    class="mb-4"
    title="Encryption"
    info="Every stored secret is encrypted at rest (AES-256-GCM). A random data key encrypts the secrets; that key is stored wrapped by the root secret, so the key that unlocks everything is never in a config file. Rotating re-encrypts every secret under a fresh key in one pass."
  />
  {#if query.isPending}
    <!-- Stat pills shimmer instead of rendering "v—/—" and reflowing. -->
    <div class="flex flex-wrap items-center gap-x-6 gap-y-1">
      {#each Array.from({ length: 4 }, (_, i) => i) as i (i)}
        <Skeleton class="h-3 w-24 rounded-full" />
      {/each}
    </div>
  {:else if !data}
    <!-- "Secrets protected: —" over a failed read looks like a fact about the
         key store rather than a fact about the request. -->
    <QueryError
      variant="inline"
      error={query.error}
      title="Could not load encryption status"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <div class="flex flex-wrap items-end gap-x-8 gap-y-2">
      <span>
        <span class="block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Key version</span>
        <span class="font-sans text-2xl font-semibold text-fg">v{data.keyVersion ?? '—'}</span>
      </span>
      <span>
        <span class="block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Secrets protected</span>
        <span class="font-sans text-2xl font-semibold text-fg">{data.secretCount}</span>
      </span>
      <span>
        <span class="block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Root of trust</span>
        <span class="font-sans text-2xl font-semibold text-fg">{data.rootSource}</span>
      </span>
      {#if data.rotatedAt}
        <span class="pb-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
          rotated {new Date(data.rotatedAt).toLocaleString()}
        </span>
      {/if}
    </div>
  {/if}
  <!-- §8 DANGER ZONE: orange hairline panel, mono label + IRREVERSIBLE
       meta, muted body, orange-outline action — never an orange fill. -->
  <div class="mt-4 rounded-md border border-danger/40 p-4">
    <div class="mb-2 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-danger">Danger zone</span>
      <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">Irreversible</span>
    </div>
    <p class="mb-3 text-xs text-muted">
      Rotating re-encrypts every stored secret under a fresh key in one pass. The previous key is retired for good.
    </p>
    <div class="flex flex-wrap items-end gap-3">
      <div class="min-w-[16rem] flex-1">
        <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">New root secret (optional)</label>
        <Input
          type="password"
          bind:value={newRoot}
          placeholder="leave blank to keep the current root"
          autocomplete="off"
        />
      </div>
      <Button size="sm" variant="danger" onclick={() => void rotate()} disabled={busy}>
        {busy ? 'Rotating' : 'Rotate keys'}
      </Button>
    </div>
    {#if busy}
      <div class="mt-3">
        <Generating
          site="admin/key-rotate"
          label="Rotating the data key: re-encrypting provider keys, agent secrets, and OAuth tokens in one pass"
          lines={2}
        />
      </div>
    {/if}
    {#if newRoot.trim()}
      <p class="mt-2 text-[11px] text-muted">
        After rotating with a new root secret, update <code class="font-mono">TALARIA_SECRET_KEY</code> (or the key-file) to match before the next restart.
      </p>
    {/if}
  </div>
  {#if msg}<p class="mt-2 text-xs text-success">{msg}</p>{/if}
</Panel>
