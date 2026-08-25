<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  /** Transactional email: bring your own SMTP (Google Workspace etc.) or
   *  connect Resend. More providers as requested. */
  interface EmailCfg {
    provider: 'smtp' | 'resend' | null
    from: string
    smtp: { host: string; port: number; secure: boolean; user: string; passSet: boolean }
    resend: { apiKeySet: boolean }
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['email-config'],
    queryFn: async (): Promise<EmailCfg> => (await getJson<{ config: EmailCfg }>('/api/admin/email')).config,
  }))
  const data = $derived(query.data)
  let error = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)
  let secret = $state('') // smtp password / resend key draft

  const post = async (body: unknown, okNotice?: string) => {
    busy = true
    error = null
    notice = null
    // PUT patches the config; POST sends the test email.
    const r = await fetch('/api/admin/email', {
      method: typeof body === 'object' && body !== null && 'test' in body ? 'POST' : 'PUT',
      credentials: 'same-origin',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    })
    busy = false
    const j = (await r.json().catch(() => ({}))) as { error?: string }
    if (!r.ok || j.error) error = j.error ?? 'failed'
    else if (okNotice) notice = okNotice
    await qc.invalidateQueries({ queryKey: ['email-config'] })
  }
</script>

{#if query.isPending}
  <Panel class="mt-4">
    <Skeleton class="mb-3 h-4 w-24 rounded-full" />
    <SkeletonRows rows={2} />
  </Panel>
{:else if !data}
  <!-- `isPending || !data` used to send a FAILED read back to the skeleton, so a
       broken /api/admin/email shimmered for ever with nothing to click. -->
  <Panel class="mt-4">
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load your email settings"
      onRetry={() => void query.refetch()}
    />
  </Panel>
{:else}
  <Panel class="mt-4">
    <SectionHeader
      title="Email"
      info="Transactional email: invites today, more later. Bring your own SMTP (e.g. Google Workspace: smtp.gmail.com, port 587, an app password) or connect Resend. Secrets are stored encrypted and never shown again."
    />
    <div class="space-y-3">
      <div class="flex items-center gap-3">
        <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Provider</label>
        <Select
          size="sm"
          value={data.provider ?? ''}
          onchange={(e) => void post({ provider: (e.currentTarget.value || null) as 'smtp' | 'resend' | null })}
          class="w-44"
        >
          <option value="">Not configured</option>
          <option value="smtp">Your own SMTP</option>
          <option value="resend">Resend</option>
        </Select>
      </div>
      {#if data.provider}
        <div class="flex items-center gap-3">
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">From</label>
          <Input
            size="sm"
            value={data.from}
            onblur={(e) => e.currentTarget.value !== data.from && void post({ from: e.currentTarget.value })}
            placeholder="Talaria &lt;talaria@yourcompany.com&gt;"
            class="w-96"
          />
        </div>
      {/if}
      {#if data.provider === 'smtp'}
        <div class="flex items-center gap-3">
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Host / port</label>
          <Input size="sm" value={data.smtp.host} onblur={(e) => void post({ smtp: { host: e.currentTarget.value } })} placeholder="smtp.gmail.com" class="w-56" />
          <Input size="sm" value={String(data.smtp.port)} onblur={(e) => void post({ smtp: { port: Number(e.currentTarget.value) || 587 } })} class="w-20" />
          <Checkbox
            title="TLS from the first byte (port 465). Off = STARTTLS (587)."
            checked={data.smtp.secure}
            onChange={(checked) => void post({ smtp: { secure: checked } })}
            label="TLS"
          />
        </div>
        <div class="flex items-center gap-3">
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">User</label>
          <Input size="sm" value={data.smtp.user} onblur={(e) => void post({ smtp: { user: e.currentTarget.value } })} placeholder="talaria@yourcompany.com" class="w-56" />
          <Input
            size="sm"
            type="password"
            bind:value={secret}
            placeholder={data.smtp.passSet ? 'password saved (type to replace)' : 'password / app password'}
            autocomplete="off"
            class="w-56"
          />
          <Button
            size="sm"
            variant="outline"
            disabled={!secret.trim() || busy}
            onclick={() => {
              void post({ smtp: { pass: secret } }, 'password saved')
              secret = ''
            }}
          >
            Save
          </Button>
        </div>
      {/if}
      {#if data.provider === 'resend'}
        <div class="flex items-center gap-3">
          <label class="w-24 shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">API key</label>
          <Input
            size="sm"
            type="password"
            bind:value={secret}
            placeholder={data.resend.apiKeySet ? 'key saved (type to replace)' : 're_…'}
            autocomplete="off"
            class="w-72"
          />
          <Button
            size="sm"
            variant="outline"
            disabled={!secret.trim() || busy}
            onclick={() => {
              void post({ resend: { apiKey: secret } }, 'key saved')
              secret = ''
            }}
          >
            Save
          </Button>
        </div>
      {/if}
      {#if data.provider}
        <div class="flex items-center gap-2 border-t border-line-subtle pt-3">
          <Button size="sm" disabled={busy} onclick={() => void post({ test: true }, 'test sent, check your inbox')}>
            {busy ? 'Working' : 'Send me a test'}
          </Button>
          {#if notice}<span class="text-xs text-success">{notice}</span>{/if}
        </div>
      {/if}
      {#if error}<div transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</div>{/if}
    </div>
  </Panel>
{/if}
