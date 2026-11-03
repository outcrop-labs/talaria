<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import PauseMenu from '@/components/notify/PauseMenu.svelte'
  import { confirm } from '@/components/ui/confirm.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import NotifyMatrix from './NotifyMatrix.svelte'
  import NotifySchedule from './NotifySchedule.svelte'
  import NotifyStatusPanel from './NotifyStatusPanel.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { cn } from '@/lib/cn'
  import {
    NOTIFY_ROUTES,
    saveNotifySettings,
    useNotifications,
    type DigestPref,
    type NotifyPrefs,
    type NotifyRoute,
  } from '@/lib/notifications'
  import { featureSet, useNotifyPatch } from '@/lib/notify-center'
  import { EMAIL_FREQUENCY_OPTIONS, mailNote } from '@/lib/notify-matrix'
  import type { EmailFrequency } from '@/lib/notify-wire'

  const DIGEST_OPTIONS: Array<{ id: DigestPref; label: string }> = [
    { id: 'on', label: 'On' },
    { id: 'off', label: 'Off' },
  ]

  // The INSTANCE-wide email master switch. Same two words as the digest control
  // because it is the same kind of question — but it is not the same kind of
  // setting, and the block it sits in says so: it is the only control on this
  // page that decides something for everybody.
  const DELIVERY_OPTIONS: Array<{ id: 'on' | 'off'; label: string }> = [
    { id: 'on', label: 'On' },
    { id: 'off', label: 'Off' },
  ]

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

<Panel as="section">
  <SectionHeader
    class="mb-4"
    title="Notifications"
    info="Every notification is kept in your inbox. Choose where else each kind reaches you: a desktop banner, a push to your browsers with Talaria closed, or email."
  />
  {#if isPending}
    <SkeletonRows rows={6} />
  {:else if !data || !digest || emailOn === null}
    <!-- "Everything is in-app only" is a claim about YOUR settings, and a
         failed read never made it. Same for "Daily digest — On", and same
         again for "email is off for this instance" — which would be a claim
         about the whole workspace. -->
    <QueryError
      variant="compact"
      error={query.error}
      title="Could not load your notification settings"
      onRetry={() => void query.refetch()}
    />
  {:else}
    <!-- The instance switch. An admin always sees it — it is the control,
         and the state it is in is half of what the rest of this page means.
         A MEMBER sees it only when it is OFF, because that is the case where
         the page below would otherwise lie to them: every Email box can be
         ticked and not one of them sends anything. When it is on, a member
         needs no explanation and gets no clutter. -->
    {#if data.canSetDelivery || !emailOn}
      <div
        class={cn(
          'mb-4 rounded-md border p-4',
          emailOn ? 'border-line bg-raised' : 'border-warning/40 bg-warning/5',
        )}
        data-email-switch={emailOn ? 'on' : 'off'}
      >
        <div class="flex items-start gap-4">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <StatusDot status={emailOn ? 'ok' : 'warn'} />
              <span class="text-sm font-medium text-fg">
                {emailOn ? 'Email delivery is on for this instance' : 'Email delivery is off for this instance'}
              </span>
            </div>
            <p class="mt-1 font-sans text-xs text-muted">
              {emailOn
                ? 'Every setting below can send mail. Switching this off stops all notification and digest email for everyone in this workspace immediately, including mail already queued and the daily digest.'
                : 'Nothing below sends mail. Notifications and the daily digest still land in the in-app inbox; none of them leave the building.'}
            </p>
            {#if data.canSetDelivery}
              <p class="mt-1 font-sans text-xs text-muted">
                This is the instance-wide switch and it overrules every personal setting on this page, for every
                user. Turning it on starts mailing the whole workspace.
              </p>
            {:else}
              <p class="mt-1 font-sans text-xs text-muted">Only an admin can turn it back on.</p>
            {/if}
          </div>
          {#if data.canSetDelivery}
            <Segmented
              options={DELIVERY_OPTIONS}
              value={emailOn ? 'on' : 'off'}
              onChange={(v) => void setDelivery(v === 'on')}
            />
          {/if}
        </div>
      </div>
    {/if}

    {#if classes}
      <NotifyMatrix {classes} channels={data.channels} emailEnabled={emailOn} onSaved={saved} onError={failed} />
    {:else if legacy}
      <ul class="divide-y divide-line-subtle">
        {#each Object.keys(legacy) as id (id)}
          <li class="flex items-center gap-4 py-3">
            <div class="min-w-0 flex-1 text-sm text-fg">{humanize(id)}</div>
            <Segmented options={NOTIFY_ROUTES} value={legacy[id] ?? 'in_app'} onChange={(r) => void setRoute(id, r)} />
          </li>
        {/each}
      </ul>
    {/if}

    <ul class="mt-4 divide-y divide-line-subtle border-t border-line-subtle">
      {#if frequency}
        <li class="flex flex-col items-start gap-2 py-3 sm:flex-row sm:items-center sm:gap-4" data-email-frequency={frequency}>
          <div class="min-w-0 flex-1">
            <div class="text-sm text-fg">Missed-activity email</div>
            <div class="font-sans text-xs text-muted">
              How often to mail what you have not seen in the app, in one email.
              {#if features.has('email-batch')}
                <!-- Said from the registry and this person's matrix: a hand-kept
                     sentence here told people Following is not mailed, while
                     Channel activity mailed by default (notify-web-4). -->
                {#if classes}{mailNote(classes, data?.channels, frequency)}{/if}
              {:else}
                Saved now; this instance starts batching by it with its next delivery update.
              {/if}
            </div>
          </div>
          <Segmented options={EMAIL_FREQUENCY_OPTIONS} value={frequency} onChange={(v) => void setFrequency(v)} />
        </li>
      {/if}
      <!-- The digest is not a class — it is one mail that summarises the
           queues — so it gets its own row and its own On/Off, below the
           matrix it does not belong to. -->
      <li class="flex flex-col items-start gap-2 py-3 sm:flex-row sm:items-center sm:gap-4">
        <div class="min-w-0 flex-1">
          <div class="text-sm text-fg">Daily digest</div>
          <div class="font-sans text-xs text-muted">
            One email a day listing what is waiting on you: approvals, tickets in review, blocked and to triage.
            Never sent on a day when nothing is waiting.
          </div>
        </div>
        <Segmented options={DIGEST_OPTIONS} value={digest} onChange={(v) => void setDigest(v)} />
      </li>
    </ul>
  {/if}
  <p class="mt-3 text-xs text-muted" aria-live="polite">
    {#if savedFlash.saved && !error}<span class="text-success">Saved</span>{/if}
    {#if error}<span class="text-danger" role="alert">{error}</span>{/if}
  </p>
</Panel>

{#if data?.dnd}
  <Panel as="section" data-notify-dnd>
    <SectionHeader
      class="mb-4"
      title="Do not disturb"
      info="Paused or outside your working hours, notifications still land in your inbox and on the bell, quietly: no banner, no push, no email."
    />
    {#if !features.has('dnd-enforced')}
      <p class="-mt-2 mb-4 max-w-xl font-sans text-xs text-muted">
        This instance saves and shows your pause and working hours, and its next delivery update enforces them for push
        and email. Until then some alerts may still arrive.
      </p>
    {/if}
    <div class="grid gap-6 md:grid-cols-[minmax(0,16rem)_minmax(0,1fr)]">
      <div>
        <div class="mb-1 font-mono text-[10px] font-semibold uppercase tracking-wide text-muted">Pause</div>
        <div class="-mx-3">
          <PauseMenu dnd={data.dnd} onDone={dndSaved} />
        </div>
      </div>
      <NotifySchedule dnd={data.dnd} onSaved={dndSaved} onError={dndFailed} />
    </div>
    <p class="mt-3 text-xs text-muted" aria-live="polite" data-dnd-status>
      {#if dndFlash.saved && !dndError}<span class="text-success">Saved</span>{/if}
      {#if dndError}<span class="text-danger" role="alert">{dndError}</span>{/if}
    </p>
  </Panel>
{/if}

<NotifyStatusPanel />
