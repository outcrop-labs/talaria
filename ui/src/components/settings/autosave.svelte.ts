// A settings page whose fields save on change says so once, in its header,
// and flashes "Saved" in the same place. `SettingsPage` provides the cue; a
// field that autosaves asks for it with `useAutosave()` and calls `flash()`
// when its save lands.
import { getContext, onDestroy, setContext } from 'svelte'

const KEY = Symbol('settings-autosave')

interface AutosaveChannel {
  register: () => void
  flash: () => void
}

/** Call in `SettingsPage`. Read `active` and `saved` off the returned object. */
export function provideAutosave(ms = 1800): { readonly active: boolean; readonly saved: boolean } {
  let active = $state(false)
  let saved = $state(false)
  let timer: ReturnType<typeof setTimeout> | null = null
  onDestroy(() => {
    if (timer) clearTimeout(timer)
  })
  setContext<AutosaveChannel>(KEY, {
    register: () => {
      active = true
    },
    flash: () => {
      saved = true
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => (saved = false), ms)
    },
  })
  return {
    get active() {
      return active
    },
    get saved() {
      return saved
    },
  }
}

/** Call during init in a component whose fields save on change or blur.
 *  Outside a `SettingsPage` it does nothing. */
export function useAutosave(): { flash: () => void } {
  const channel = getContext<AutosaveChannel | undefined>(KEY)
  channel?.register()
  return { flash: () => channel?.flash() }
}
