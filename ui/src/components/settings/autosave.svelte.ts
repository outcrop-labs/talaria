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

interface Autosave {
  readonly active: boolean
  readonly saved: boolean
  channel: AutosaveChannel
  dispose: () => void
}

/** The cue's state with no component around it: `provideAutosave` is this in a page's context. */
export function createAutosave(ms = 1800): Autosave {
  // A count, not a flag: a tabbed page mounts and unmounts autosaving fields
  // under one header, and the cue must go when the last of them does. The
  // count is plain: a field releases in a teardown, where Svelte reads state
  // as it was before the update, so a reactive count loses the fields the
  // next tab mounted in the same update.
  let fields = 0
  let active = $state(false)
  let saved = $state(false)
  let timer: ReturnType<typeof setTimeout> | null = null
  return {
    get active() {
      return active
    },
    get saved() {
      return saved
    },
    channel: {
      register: () => {
        fields += 1
        active = true
      },
      release: () => {
        fields -= 1
        active = fields > 0
      },
      flash: () => {
        saved = true
        if (timer) clearTimeout(timer)
        timer = setTimeout(() => (saved = false), ms)
      },
    },
    dispose: () => {
      if (timer) clearTimeout(timer)
    },
  }
}

/** Call in `SettingsPage`. Read `active` and `saved` off the returned object. */
export function provideAutosave(ms = 1800): { readonly active: boolean; readonly saved: boolean } {
  const cue = createAutosave(ms)
  onDestroy(cue.dispose)
  setContext<AutosaveChannel>(KEY, cue.channel)
  return cue
}

/** Call during init in a component whose fields save on change or blur.
 *  Outside a `SettingsPage` it does nothing. `when` is for a component that
 *  only sometimes shows such fields (nothing set up yet, a viewer who cannot
 *  edit): the cue is on while it returns true. */
export function useAutosave(when?: () => boolean): { flash: () => void } {
  const channel = getContext<AutosaveChannel | undefined>(KEY)
  if (when) {
    $effect(() => {
      if (!when()) return
      channel?.register()
      return () => channel?.release()
    })
  } else {
    channel?.register()
    onDestroy(() => channel?.release())
  }
  return { flash: () => channel?.flash() }
}
