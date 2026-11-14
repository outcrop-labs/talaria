// A settings page whose fields save on change says so once, in its header,
// and flashes "Saved" in the same place. `SettingsPage` provides the cue; a
// field that autosaves asks for it with `useAutosave()` and calls `flash()`
// when its save lands.
import { getContext, onDestroy, setContext } from 'svelte'

const KEY = Symbol('settings-autosave')

interface AutosaveChannel {
  register: () => void
  release: () => void
  flash: () => void
}

/** Call in `SettingsPage`. Read `active` and `saved` off the returned object. */
export function provideAutosave(ms = 1800): { readonly active: boolean; readonly saved: boolean } {
  // A count, not a flag: a tabbed page mounts and unmounts autosaving fields
  // under one header, and the cue must go when the last of them does.
  let fields = $state(0)
  let saved = $state(false)
  let timer: ReturnType<typeof setTimeout> | null = null
  onDestroy(() => {
    if (timer) clearTimeout(timer)
  })
  setContext<AutosaveChannel>(KEY, {
    register: () => {
      fields += 1
    },
    release: () => {
      fields -= 1
    },
    flash: () => {
      saved = true
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => (saved = false), ms)
    },
  })
  return {
    get active() {
      return fields > 0
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
  onDestroy(() => channel?.release())
  return { flash: () => channel?.flash() }
}
