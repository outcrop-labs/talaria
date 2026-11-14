<script lang="ts">
  import { createQuery, useQueryClient } from '@tanstack/svelte-query'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsRowMenu from '@/components/settings/SettingsRowMenu.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import QueryState from '@/components/ui/QueryState.svelte'
  import { delJson, errorMessage, getJson } from '@/lib/fetch-json'
  import { slide } from '@/lib/motion'

  // The assistant's Google Workspace connection, as rows of the access group on
  // Settings › Assistant: the grant that makes "manage my mail and calendar"
  // real. The same connection also powers doc export on the Connections tab.
  interface GoogleStatus {
    available: boolean
    connected: boolean
    email: string | null
    connectedAt: string | null
  }

  const qc = useQueryClient()
  const query = createQuery(() => ({
    queryKey: ['integration-google'],
    queryFn: (): Promise<GoogleStatus> => getJson<GoogleStatus>('/api/integrations/google'),
  }))
  let error = $state<string | null>(null)

  const disconnect = async () => {
    if (
      !(await confirm({
        title: 'Disconnect Google',
        message: 'Disconnect your Google Workspace account? Your assistant loses mail, calendar, and Drive access until you reconnect.',
        confirmLabel: 'Disconnect',
        danger: true,
      }))
    )
      return
    error = null
    try {
      await delJson('/api/integrations/google')
    } catch (e) {
      error = errorMessage(e)
    }
    await qc.invalidateQueries({ queryKey: ['integration-google'] })
  }
</script>

<QueryState query={query} errorTitle="Could not load the Google connection" errorVariant="compact" isEmpty={(d) => !d.available}>
  {#snippet skeleton()}
    <!-- Hold the row's shape until the status resolves: "Not connected" is a
         claim about the account, and an in-flight read can't make it. -->
    <div aria-hidden="true" class="flex min-h-[52px] items-center gap-3 px-5 py-3">
      <div class="min-w-0 flex-1 space-y-2">
        <Skeleton class="h-3 w-36 rounded-full" />
        <Skeleton class="h-2.5 w-52 rounded-full" />
      </div>
      <Skeleton class="h-8 w-24 rounded-md" />
    </div>
  {/snippet}
  {#snippet empty()}
    <!-- An admin hasn't registered the OAuth client. The fix is theirs, and the
         sentence says whose it is. -->
    <SettingsRow label="Google Workspace" hint="Not set up on this server yet. Ask an admin to register the Google client in Admin." />
  {/snippet}
  {#snippet children(data)}
    <SettingsRow
      label="Google Workspace"
      hint={data.connected ? (data.email ?? 'Your account') : 'Mail, Calendar and Drive.'}
      info="It reads as soon as you connect. Every email and invite it drafts waits for your approval in the Inbox."
    >
      <!-- Beside its control: in a group of settings, a status column of one would sit apart from everything. -->
      <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
        <StatusDot status={data.connected ? 'ok' : 'idle'} />{data.connected ? 'Connected' : 'Not connected'}
      </span>
      {#if data.connected}
        <SettingsRowMenu
          label="Actions for Google Workspace"
          items={[{ label: 'Disconnect Google', danger: true, onSelect: () => void disconnect() }]}
        />
      {:else}
        <a href="/api/integrations/google/connect" class={buttonClasses({ size: 'sm', variant: 'outline', voice: 'sans' })}>
          Connect
        </a>
      {/if}
    </SettingsRow>
  {/snippet}
</QueryState>
{#if error}
  <p transition:slide={{ duration: 150 }} class="px-5 py-3 font-sans text-[13px] leading-5 text-danger" role="alert">{error}</p>
{/if}
