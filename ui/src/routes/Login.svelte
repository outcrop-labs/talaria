<script lang="ts">
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import LoginScreen from '@/components/auth/LoginScreen.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import { useSession } from '@/lib/session'

  const session = useSession()
  // searchParams.get returns string | number | boolean | null; the login
  // screen wants string | undefined.
  const rawError = $derived(searchParams.get('error'))
  const error = $derived(rawError == null ? undefined : String(rawError))
  const rawDomain = $derived(searchParams.get('domain'))
  const domain = $derived(rawDomain == null ? undefined : String(rawDomain))

  // Already signed in → straight to the cockpit.
  $effect(() => {
    if (session.isSuccess && session.data) navigate('/')
  })
</script>

<MercuryBackdrop />
<LoginScreen {error} {domain} />
