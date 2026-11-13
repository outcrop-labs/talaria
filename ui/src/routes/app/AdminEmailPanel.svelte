<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SaveAction from '@/components/admin/SaveAction.svelte'
  import { secretHint } from '@/components/admin/secret-hint'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import Select from '@/components/ui/Select.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import { errorMessage, getJson, postJson, putJson } from '@/lib/fetch-json'
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
  const autosave = useAutosave()
  const query = createQuery(() => ({
    queryKey: ['email-config'],
    queryFn: async (): Promise<EmailCfg> => (await getJson<{ config: EmailCfg }>('/api/admin/email')).config,
  }))
  let error = $state<string | null>(null)
  let notice = $state<string | null>(null)
  let busy = $state(false)
  let secret = $state('') // smtp password / resend key draft

  const post = async (body: unknown, okNotice?: string) => {
    busy = true
    error = null
    notice = null
    try {
      // PUT patches the config; POST sends the test email.
      const send = typeof body === 'object' && body !== null && 'test' in body ? postJson : putJson
      await send<{ ok: true }>('/api/admin/email', body)
      // A field that saved on change flashes the header's cue. A secret and the
      // test email answer beside their own button.
      if (okNotice) notice = okNotice
      else autosave.flash()
    } catch (e) {
      error = errorMessage(e)
    }
    busy = false
    await qc.invalidateQueries({ queryKey: ['email-config'] })
  }
</script>

<SettingsSection
  title="Email"
  description="How Talaria sends invites and notifications."
  info="Use your own mail server, for example Google Workspace with smtp.gmail.com, port 587 and an app password, or connect Resend. Passwords and keys are stored encrypted and never shown again."
>
  <SettingsGroup>
    <!-- `isPending || !data` used to send a FAILED read back to the skeleton, so
         a broken /api/admin/email shimmered for ever with nothing to click.
         QueryState's error branch holds it instead. -->
    <QueryState query={query} errorTitle="Could not load your email settings" errorVariant="compact">
      {#snippet skeleton()}
        <SettingsRow label="Provider"><Skeleton class="h-9 w-[280px]" /></SettingsRow>
        <SettingsRow label="From address"><Skeleton class="h-9 w-[280px]" /></SettingsRow>
      {/snippet}
      {#snippet children(data)}
        <SettingsRow label="Provider" hint="The service that delivers the mail." as="label">
          <Select
            size="sm"
            value={data.provider ?? ''}
            onchange={(e) => void post({ provider: (e.currentTarget.value || null) as 'smtp' | 'resend' | null })}
            class="w-[280px]"
          >
            <option value="">Not set up</option>
            <option value="smtp">Your own mail server</option>
            <option value="resend">Resend</option>
          </Select>
        </SettingsRow>
        {#if data.provider}
          <SettingsRow label="From address" hint="The sender people see." as="label">
            <Input
              size="sm"
              value={data.from}
              onblur={(e) => e.currentTarget.value !== data.from && void post({ from: e.currentTarget.value })}
              placeholder="Talaria <talaria@yourcompany.com>"
              class="w-[280px]"
            />
          </SettingsRow>
        {/if}
        {#if data.provider === 'smtp'}
          <SettingsRow label="Server" hint="Host name and port.">
            <Input
              size="sm"
              aria-label="Mail server host"
              value={data.smtp.host}
              onblur={(e) => e.currentTarget.value !== data.smtp.host && void post({ smtp: { host: e.currentTarget.value } })}
              placeholder="smtp.gmail.com"
              class="w-[196px] font-mono"
            />
            <Input
              size="sm"
              aria-label="Mail server port"
              value={String(data.smtp.port)}
              onblur={(e) => (Number(e.currentTarget.value) || 587) !== data.smtp.port && void post({ smtp: { port: Number(e.currentTarget.value) || 587 } })}
              class="w-[72px] font-mono"
            />
          </SettingsRow>
          <SettingsRow label="Encrypt from the first byte" hint="On for port 465. Off uses STARTTLS, usually on port 587.">
            <Toggle
              title="Encrypt from the first byte"
              checked={data.smtp.secure}
              onChange={(checked) => void post({ smtp: { secure: checked } })}
            />
          </SettingsRow>
          <SettingsRow label="Username" as="label">
            <Input
              size="sm"
              value={data.smtp.user}
              onblur={(e) => e.currentTarget.value !== data.smtp.user && void post({ smtp: { user: e.currentTarget.value } })}
              placeholder="talaria@yourcompany.com"
              class="w-[280px]"
            />
          </SettingsRow>
          <SettingsRow
            label="Password"
            hint={secretHint(data.smtp.passSet, 'password')}
          >
            <Input
              size="sm"
              type="password"
              aria-label="Mail server password"
              bind:value={secret}
              placeholder={data.smtp.passSet ? 'New password' : 'Password'}
              autocomplete="off"
              class="w-[280px]"
            />
            <SaveAction
              size="md"
              dirty={!!secret.trim()}
              {busy}
              onclick={() => {
                void post({ smtp: { pass: secret } }, 'Password saved.')
                secret = ''
              }}
            >
              Save password
            </SaveAction>
          </SettingsRow>
        {/if}
        {#if data.provider === 'resend'}
          <SettingsRow
            label="API key"
            hint={secretHint(data.resend.apiKeySet)}
          >
            <Input
              size="sm"
              type="password"
              aria-label="Resend API key"
              bind:value={secret}
              placeholder={data.resend.apiKeySet ? 'New key' : 're_'}
              autocomplete="off"
              class="w-[280px]"
            />
            <SaveAction
              size="md"
              dirty={!!secret.trim()}
              {busy}
              onclick={() => {
                void post({ resend: { apiKey: secret } }, 'Key saved.')
                secret = ''
              }}
            >
              Save key
            </SaveAction>
          </SettingsRow>
        {/if}
        {#if data.provider}
          <SettingsRow label="Test email" hint="Sends one message to your own address.">
            {#if notice}<span class="font-sans text-[13px] leading-5 text-success">{notice}</span>{/if}
            <Button size="sm" variant="outline" disabled={busy} onclick={() => void post({ test: true }, 'Sent. Check your inbox.')}>
              {busy ? 'Working' : 'Send test'}
            </Button>
          </SettingsRow>
        {/if}
        {#if error}
          <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger">{error}</p>
        {/if}
      {/snippet}
    </QueryState>
  </SettingsGroup>
</SettingsSection>
