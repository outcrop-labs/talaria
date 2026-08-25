<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { confirm } from '@/components/ui/confirm.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Segmented from '@/components/ui/Segmented.svelte'
  import BrowserNotifications from './BrowserNotifications.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { cn } from '@/lib/cn'
  import {
    NOTIFY_CLASSES,
    NOTIFY_ROUTES,
    saveNotifySettings,
    useNotifications,
    type DigestPref,
    type NotifyClass,
    type NotifyPrefs,
    type NotifyRoute,
  } from '@/lib/notifications'

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

  // Where each class of event reaches you, and whether the daily digest comes at
  // all. Autosaves on change (UI-CONVENTIONS: pickers apply immediately) and
  // shows the server's effective answer, so a rejected save can never look like
  // it landed — and neither can a control that governs nothing.

  const qc = useQueryClient()
  const query = useNotifications()
  const data = $derived(query.data)
  const isPending = $derived(query.isPending)
  // Optimistic overlay: the segmented control must move under the cursor, but
  // the truth stays whatever the server last returned.
  let pending = $state<Partial<NotifyPrefs>>({})
  let pendingDigest = $state<DigestPref | null>(null)
  let pendingDelivery = $state<boolean | null>(null)
  let error = $state<string | null>(null)
  const savedFlash = useSavedFlash()
  const prefs = $derived(data ? { ...data.prefs, ...pending } : null)
  const digest: DigestPref | null = $derived(data ? (pendingDigest ?? data.digest) : null)
  const emailOn: boolean | null = $derived(data ? (pendingDelivery ?? data.delivery.emailEnabled) : null)

  const set = async (id: NotifyClass, route: NotifyRoute) => {
    error = null
    pending = { ...pending, [id]: route }
    const r = await saveNotifySettings({ prefs: { [id]: route } })
    if ('error' in r) {
      // Roll the control back to the server's value — a setting that silently
      // failed is worse than one that visibly refused.
      const next = { ...pending }
      delete next[id]
      pending = next
      error = r.error
      return
    }
    await qc.invalidateQueries({ queryKey: ['notifications'] })
    pending = {}
    savedFlash.flash()
  }

  // The digest switch. Same contract as the class pickers: optimistic, rolled
  // back on refusal, and the server's effective answer is what stays on screen
  // — routing every class to in-app also turns the digest off by derivation, so
  // this control has to re-read rather than assume the value it just sent.
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
    savedFlash.flash()
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
    savedFlash.flash()
  }
</script>

<Panel as="section">
  <SectionHeader
    class="mb-4"
    title="Notifications"
    info="Email goes out when a person is the only one who can unblock something. Outcomes stay in the app, where you'll see them next time you look."
  />
  {#if isPending}
    <SkeletonRows rows={6} />
  {:else if !data || !prefs || !digest || emailOn === null}
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
         the page below would otherwise lie to them: every row can say
         "Email" and not one of them sends anything. When it is on, a member
         needs no explanation and gets no clutter. -->
    {#if data.canSetDelivery || !emailOn}
      <div
        class={cn(
          'mb-4 rounded-md border p-4',
          emailOn ? 'border-line bg-raised' : 'border-warning/40 bg-warning/5',
        )}
      >
        <div class="flex items-start gap-4">
          <div class="min-w-0 flex-1">
            <div class="flex items-center gap-2">
              <span
                aria-hidden="true"
                class={cn('h-1.5 w-1.5 shrink-0 rounded-full', emailOn ? 'bg-success' : 'bg-warning')}
              ></span>
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
    <ul class="divide-y divide-line-subtle">
      {#each NOTIFY_CLASSES as c (c.id)}
        <li class="flex items-center gap-4 py-3">
          <div class="min-w-0 flex-1">
            <div class="text-sm text-fg">{c.label}</div>
            <div class="font-sans text-xs text-muted">{c.blurb}</div>
          </div>
          <Segmented
            options={NOTIFY_ROUTES.map((r) => ({ id: r.id, label: r.label }))}
            value={prefs[c.id]}
            onChange={(r) => void set(c.id, r)}
          />
        </li>
      {/each}
      <!-- The digest is not a class — it is one mail that summarises the
           queues — so it gets its own row and its own On/Off, below the
           routing table it does not belong to. -->
      <li class="flex items-center gap-4 py-3">
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
  <!-- The one destination the server can't see or route: the person's own
       browser. Client-only, so it renders regardless of the server settings
       above. -->
  <BrowserNotifications />
  <p class="mt-3 text-xs text-muted">
    Every notification is kept in your inbox either way. “Email” means it arrives already read, so you aren’t chased
    twice for the same thing.
    {#if savedFlash.saved && !error}<span class="ml-2 text-success">Saved</span>{/if}
    {#if error}<span class="ml-2 text-danger">{error}</span>{/if}
  </p>
</Panel>
