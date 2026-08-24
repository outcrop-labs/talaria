<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useSession } from '@/lib/session'
  import { saveTimezone, useProfilePrefs } from '@/lib/muse.svelte'
  import { detectedZone, isValidTimeZone } from '@/lib/timezone'
  import { hasAdopted, markTimezoneAdopted } from '@/lib/timezone-adopt'
  import { BRIEF_KEY } from '@/components/brief/daily-brief.svelte'

  // Silent first-run timezone adoption: when a signed-in person has never set
  // a zone, the browser's is saved once, behind their back but in their
  // interest — briefs and digests are wrong by whole days until SOME zone is
  // known, and asking everyone to find a setting first is a tax the browser
  // already knows the answer to.
  //
  // THE TWO THINGS IT MUST NEVER DO: adopt over a set zone (a laptop in an
  // airport lounge does not outrank a person's explicit choice — travel must
  // not fight the setting), and re-adopt after a deliberate clear (that is
  // what the localStorage flag in lib/timezone-adopt.ts is for).
  //
  // Renders nothing. Mounted once by AppLayout, so every session-gated
  // surface adopts; public pages have no session and skip it. The prefs query
  // it mounts is the one Settings reads anyway — one cached GET, and Settings
  // is pre-warmed.

  const qc = useQueryClient()
  const session = useSession()
  const prefs = useProfilePrefs()
  let adopting = $state(false)

  $effect(() => {
    const userId = session.data?.id
    const stored = prefs.data?.timezone
    // `undefined` while the prefs read is in flight — adopting on that would
    // save a zone over an answer that simply has not arrived.
    if (!userId || stored === undefined || adopting) return
    const detected = detectedZone()
    if (!detected || !isValidTimeZone(detected)) return
    if (stored !== null || hasAdopted(userId)) return
    adopting = true
    void saveTimezone(detected).then(async () => {
      markTimezoneAdopted(userId)
      // The brief may already be on screen, computed under the workspace
      // zone — invalidate so its next read happens under the person's own.
      await qc.invalidateQueries({ queryKey: ['profile-prefs'] })
      await qc.invalidateQueries({ queryKey: BRIEF_KEY })
      adopting = false
    })
  })
</script>

<!-- Intentionally empty: this component is an effect, not a surface. -->
