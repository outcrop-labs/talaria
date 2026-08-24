<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Combobox from '@/components/ui/Combobox.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'
  import { saveTimezone, useProfilePrefs } from '@/lib/muse.svelte'
  import { detectedZone, isValidTimeZone, supportedTimeZones, zoneNowLabel } from '@/lib/timezone'
  import { markTimezoneAdopted } from '@/lib/timezone-adopt'
  import { BRIEF_KEY } from '@/components/brief/daily-brief.svelte'
  import { useSession } from '@/lib/session'

  // The person's time zone — the clock their daily brief opens on and their
  // digest arrives on. Null (the "Workspace default" row) follows the
  // workspace setting; everything else is an IANA name the server validates.
  //
  // The first-run adopter (components/app/TimezoneAdopt.svelte) usually fills
  // this in before a person ever opens Settings, so the picker most often
  // CONFIRMS a choice the browser already made — which is also why picking
  // "Workspace default" here marks the adopt flag: a deliberate clear must
  // outlive the next reload, or adoption treats it as a blank to refill.

  const qc = useQueryClient()
  const prefsQuery = useProfilePrefs()
  // Same session query AppLayout already mounted (same key) — a cache read.
  const session = useSession()
  const prefs = $derived(prefsQuery.data)
  const loading = $derived(prefsQuery.isPending)
  const savedFlash = useSavedFlash()
  let error = $state<string | null>(null)

  const detected = $derived((() => {
    const z = detectedZone()
    return z && isValidTimeZone(z) ? z : null
  })())

  const zones = $derived(supportedTimeZones())

  const save = async (zone: string | null) => {
    error = null
    const r = await saveTimezone(zone)
    if (r && typeof r === 'object' && 'error' in r && r.error) error = String(r.error)
    // A deliberate clear must outlive the next reload, or the adopter treats
    // it as a blank to refill with the browser's zone.
    if (zone === null && session.data?.id) markTimezoneAdopted(session.data.id)
    await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
    // An open brief page is showing times and a document computed under the
    // old zone; the next read must happen under the new one.
    await qc.invalidateQueries({ queryKey: BRIEF_KEY })
    savedFlash.flash()
  }
</script>

<div class="mt-5 border-t border-line-subtle pt-4">
  <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Time zone</label>
  {#if loading}
    <Skeleton class="h-11 w-full" />
  {:else if prefsQuery.isError}
    <QueryError
      variant="inline"
      error={prefsQuery.error}
      title="Could not load your time zone"
      onRetry={() => void prefsQuery.refetch()}
    />
  {:else}
    <Combobox
      options={[
        { value: '', label: 'Workspace default', sub: 'Follow the workspace time zone setting' },
        ...(detected && detected !== prefs?.timezone
          ? [{ value: detected, label: detected, sub: 'Detected on this device' }]
          : []),
        ...zones.map((z) => ({ value: z, label: z, sub: zoneNowLabel(z) })),
      ]}
      selected={[prefs?.timezone ?? '']}
      onChange={([v]) => void save(v || null)}
      placeholder="Pick a time zone"
    />
  {/if}
  <p class="mt-1 text-xs text-muted">
    When your daily brief opens and your digest arrives. Detected: {detected ?? '—'}
    {#if savedFlash.saved && !error}<span class="ml-2 text-success">Saved</span>{/if}
    {#if error}<span class="ml-2 text-danger">{error}</span>{/if}
  </p>
</div>
