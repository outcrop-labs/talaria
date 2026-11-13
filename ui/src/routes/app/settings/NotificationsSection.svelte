<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import PauseMenu from '@/components/notify/PauseMenu.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import Toggle from '@/components/ui/Toggle.svelte'
  import Button from '@/components/ui/Button.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import NotifyMatrix from './NotifyMatrix.svelte'
  import NotifySchedule from './NotifySchedule.svelte'
  import NotifyStatusPanel from './NotifyStatusPanel.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import {
    NOTIFY_ROUTES,
    saveNotifySettings,
    useNotifications,
    type DigestPref,
    type NotifyPrefs,
    type NotifyRoute,
  } from '@/lib/notifications'
  import { featureSet, useNotifyPatch } from '@/lib/notify-center'
  import { dndView, safeZone } from '@/lib/notify-dnd'
  import { EMAIL_FREQUENCY_OPTIONS, mailNote } from '@/lib/notify-matrix'
  import type { EmailFrequency } from '@/lib/notify-wire'

  // Settings → Notifications: what reaches you where (the category × channel
  // matrix, its rows the server's class registry), how often missed activity
  // is mailed, the daily digest, Do Not Disturb (pause and working hours), and
  // the status panel that says why something did or did not arrive. Every
  // control autosaves on change (UI-CONVENTIONS: pickers apply immediately)
  // and shows the server's effective answer, so a rejected save can never look
  // like it landed — and neither can a control that governs nothing.

  const qc = useQueryClient()
  const query = useNotifications()
  const patch = useNotifyPatch()
  const data = $derived(query.data)
  const isPending = $derived(query.isPending)
  let pendingDigest = $state<DigestPref | null>(null)
  let pendingDelivery = $state<boolean | null>(null)
  let pendingFrequency = $state<EmailFrequency | null>(null)
  let pendingRoutes = $state<Partial<NotifyPrefs>>({})
  let error = $state<string | null>(null)
  const savedFlash = useSavedFlash()
  const digest: DigestPref | null = $derived(data ? (pendingDigest ?? data.digest) : null)
  const emailOn: boolean | null = $derived(data ? (pendingDelivery ?? data.delivery.emailEnabled) : null)
  const frequency = $derived(pendingFrequency ?? data?.emailFrequency ?? null)
  // The matrix needs the registry. An api older than the contract serves only
  // the legacy routes, and those still get their wave-1 control below.
  const classes = $derived(data?.classes)
  // What the delivery engine enforces (W2-E's flags): until it does, the
  // frequency and DND are stored and shown, and the page says so.
  const features = $derived(featureSet(data))
  const legacy = $derived(data && !classes ? { ...data.prefs, ...pendingRoutes } : null)

  const saved = () => {
    error = null
    savedFlash.flash()
  }
  const failed = (m: string) => {
    error = m
  }
  // Do Not Disturb is its own panel, further down the page: its saves and
  // refusals are said there, where the person clicked, not in the panel above.
  let dndError = $state<string | null>(null)
  const dndFlash = useSavedFlash()
  const dndSaved = () => {
    dndError = null
    dndFlash.flash()
  }
  const dndFailed = (m: string) => {
    dndError = m
  }
  const dndNow = $derived(dndView(data?.dnd, Date.now(), safeZone(data?.dnd?.timeZone)))
  const resume = async () => {
    const r = await patch({ dnd: { pausedUntil: null } })
    if ('error' in r) dndFailed(r.error)
    else dndSaved()
  }

  // The digest switch: optimistic, rolled back on refusal, and the server's
  // effective answer is what stays on screen — turning email off for every
  // original class also turns the digest off by derivation (C27), so this
  // control re-reads rather than assumes the value it just sent.
  const setDigest = async (next: DigestPref) => {
    error = null
    pendingDigest = next
    const r = await saveNotifySettings({ digest: next })
    if ('error' in r) {
      pendingDigest = null
      error = r.error
      return
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    pendingDigest = null
    saved()
  }

  const setFrequency = async (next: EmailFrequency) => {
    error = null
    pendingFrequency = next
    const r = await patch({ emailFrequency: next })
    pendingFrequency = null
    if ('error' in r) error = r.error
    else saved()
  }

  // The legacy route control, for an api with no registry.
  const setRoute = async (id: string, route: NotifyRoute) => {
    error = null
    pendingRoutes = { ...pendingRoutes, [id]: route }
    const r = await saveNotifySettings({ prefs: { [id]: route } })
    const next = { ...pendingRoutes }
    delete next[id]
    pendingRoutes = next
    if ('error' in r) {
      error = r.error
      return
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    saved()
  }

  // THE MASTER SWITCH. Admin-only, and until this control existed it was
  // reachable only by hand-crafting a PATCH — an emergency switch you need a
  // terminal and the route source to throw is not an emergency switch.
  //
  // Asymmetric on purpose. Turning it ON starts mailing every user in the
  // workspace, so it asks first. Turning it OFF is the direction you reach for
  // when something is already going wrong, and a confirmation dialog in front
  // of a kill switch is a delay measured in mails.
  const setDelivery = async (next: boolean) => {
    error = null
    if (
      next &&
      !(await confirm({
        title: 'Turn on email for this whole instance',
        message:
          'Talaria will start sending notification and digest email to every user in this workspace, ' +
          'following each person’s own settings, including people who have never opened this page. ' +
          'Check the provider, the From address and the sending domain under Admin → Org → Email first.',
        confirmLabel: 'Turn email on',
      }))
    ) {
      return
    }
    pendingDelivery = next
    const r = await saveNotifySettings({ delivery: { emailEnabled: next } })
    if ('error' in r) {
      // Rolled back to the server's answer. A master switch that looks flipped
      // and is not is the worst possible version of this control.
      pendingDelivery = null
      error = r.error
      return
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    pendingDelivery = null
    saved()
  }

  const humanize = (id: string) => id.replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase())
</script>

{#if isPending}
  <SettingsGroup><div class="px-5 py-4"><SkeletonRows rows={6} /></div></SettingsGroup>
{:else if !data || !digest || emailOn === null}
  <!-- "Everything is in-app only" is a claim about YOUR settings, and a
       failed read never made it. Same for "Daily digest: on", and same again
       for "email is off for this instance", which would be a claim about the
       whole workspace. -->
  <QueryError
    variant="compact"
    error={query.error}
    title="Could not load your notification settings"
    onRetry={() => void query.refetch()}
  />
{:else}
  <!-- The instance switch. An admin always sees it: it is the control, and
       the state it is in is half of what the rest of this page means. A
       MEMBER sees it only when it is OFF, because that is the case where the
       page below would otherwise lie to them: every Email box can be ticked
       and not one of them sends anything. -->
  {#if data.canSetDelivery || !emailOn}
    <SettingsGroup>
      <div data-email-switch={emailOn ? 'on' : 'off'}>
        <SettingsRow
          label="Email for this instance"
          hint={data.canSetDelivery
            ? 'Sends notification and digest email to everyone in this workspace.'
            : 'Nothing below sends mail. Only an admin can turn it back on.'}
          info={data.canSetDelivery
            ? 'The instance-wide switch. It overrules every personal setting on this page, for every user. Off stops all notification and digest email at once, including mail already queued.'
            : undefined}
        >
          <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted">
            <StatusDot status={emailOn ? 'ok' : 'warn'} />{emailOn ? 'On' : 'Off'}
          </span>
          {#if data.canSetDelivery}
            <Toggle checked={emailOn} onChange={(on) => void setDelivery(on)} title="Email for this instance" />
          {/if}
        </SettingsRow>
      </div>
    </SettingsGroup>
  {/if}

  {#if classes || (legacy && Object.keys(legacy).length > 0)}
  <SettingsSection title="What reaches you" description="Everything is kept in your inbox. Choose where else each kind goes.">
    <SettingsGroup>
      {#if classes}
        <NotifyMatrix {classes} channels={data.channels} emailEnabled={emailOn} onSaved={saved} onError={failed} />
      {:else if legacy}
        {#each Object.keys(legacy) as id (id)}
          <SettingsRow label={humanize(id)}>
            <Segmented options={NOTIFY_ROUTES} value={legacy[id] ?? 'in_app'} onChange={(r) => void setRoute(id, r)} />
          </SettingsRow>
        {/each}
      {/if}
    </SettingsGroup>
  </SettingsSection>
  {/if}

  <SettingsSection title="Email">
    <SettingsGroup>
      {#if frequency}
        <div data-email-frequency={frequency}>
          <SettingsRow label="Missed-activity email">
            {#snippet hint()}
              One email for what you have not seen in the app.
              {#if features.has('email-batch')}
                <!-- Said from the registry and this person's matrix: a hand-kept
                     sentence here told people Following is not mailed, while
                     Channel activity mailed by default (notify-web-4). -->
                {#if classes}{mailNote(classes, data?.channels, frequency)}{/if}
              {:else}
                Saved now. Batching starts with this instance's next update.
              {/if}
            {/snippet}
            <Segmented options={EMAIL_FREQUENCY_OPTIONS} value={frequency} onChange={(v) => void setFrequency(v)} />
          </SettingsRow>
        </div>
      {/if}
      <!-- The digest is not a class: it is one mail that summarises the
           queues, so it gets its own row below the matrix it does not belong to. -->
      <SettingsRow label="Daily digest" hint="One email a day listing what is waiting on you. Skipped when nothing is.">
        <Toggle checked={digest === 'on'} onChange={(on) => void setDigest(on ? 'on' : 'off')} title="Daily digest" />
      </SettingsRow>
    </SettingsGroup>
    <p class="mt-2 font-sans text-[13px] leading-5 text-muted" aria-live="polite">
      {#if savedFlash.saved && !error}<span class="text-success">Saved</span>{/if}
      {#if error}<span class="text-danger" role="alert">{error}</span>{/if}
    </p>
  </SettingsSection>
{/if}

{#if data?.dnd}
  <div data-notify-dnd>
    <SettingsSection
      title="Do not disturb"
      description={features.has('dnd-enforced')
        ? 'Paused or outside your hours, notifications still land in your inbox, quietly.'
        : 'Saved now. This instance starts enforcing pause and working hours with its next update.'}
    >
      <SettingsGroup>
        <div data-status-pause={dndNow.active ? dndNow.reason : 'off'}>
          <SettingsRow label="Pause" hint={dndNow.active && dndNow.reason === 'paused' ? (dndNow.label ?? 'Paused') : 'Not paused'}>
            {#if dndNow.active && dndNow.reason === 'paused'}
              <Button size="sm" variant="ghost" onclick={() => void resume()}>Resume</Button>
            {/if}
            <DropdownMenu items={[]}>
              {#snippet trigger()}
                <Button size="sm" variant="outline">Pause notifications</Button>
              {/snippet}
              {#snippet content(close)}
                <div class="w-64 font-sans">
                  <PauseMenu
                    dnd={data.dnd}
                    onDone={() => {
                      dndSaved()
                      close()
                    }}
                  />
                </div>
              {/snippet}
            </DropdownMenu>
          </SettingsRow>
        </div>
        <NotifySchedule dnd={data.dnd} onSaved={dndSaved} onError={dndFailed} />
      </SettingsGroup>
      <p class="mt-2 font-sans text-[13px] leading-5 text-muted" aria-live="polite" data-dnd-status>
        {#if dndFlash.saved && !dndError}<span class="text-success">Saved</span>{/if}
        {#if dndError}<span class="text-danger" role="alert">{dndError}</span>{/if}
      </p>
    </SettingsSection>
  </div>
{/if}

<NotifyStatusPanel />
