<script lang="ts">
  import { searchParams } from 'sv-router'
  import { navigate } from '@/router'
  import LoginScreen from '@/components/auth/LoginScreen.svelte'
  import MercuryBackdrop from '@/components/MercuryBackdrop.svelte'
  import { useSession } from '@/lib/session'

  const session = useSession()
  const error = $derived(searchParams.get('error') ?? undefined)

  // Already signed in → straight to the cockpit.
  $effect(() => {
    if (session.isSuccess && session.data) navigate('/')
  })
</script>

<MercuryBackdrop />
<LoginScreen {error} />
