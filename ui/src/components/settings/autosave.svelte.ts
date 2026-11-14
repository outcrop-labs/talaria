// How a settings page says a change was saved. A page whose controls all
// save on change says so once, in its header, and flashes "Saved" there. A
// page that also holds a group with its own Save button shows no header cue:
// each control that saves on change flashes "Saved" beside itself instead.
// `SettingsPage` provides the header; a control asks with `useAutosave()`,
// and a group with a button declares itself with `useExplicitSave()` (which
// `SettingsSaveBar` calls).
import { getContext, onDestroy, setContext } from 'svelte'

const KEY = Symbol('settings-autosave')

interface AutosaveChannel {
  register: () => void
  release: () => void
  hold: () => void
  drop: () => void
  /** Flash the header's "Saved". False when the header shows no cue, so the
   *  control that saved flashes its own row. */
  flash: () => boolean
}

interface Autosave {
  /** The header shows the cue. */
  readonly active: boolean
  readonly saved: boolean
  channel: AutosaveChannel
  dispose: () => void
}

/** The cue's state with no component around it: `provideAutosave` is this in
 *  a page's context. `declared` is the page saying all its controls save on
 *  change (`SettingsPage autosave`). */
export function createAutosave(ms = 1800, declared: () => boolean = () => false): Autosave {
  // Counts, not flags: a tabbed page mounts and unmounts autosaving fields
  // under one header, and the cue must go when the last of them does. The
  // counts are plain: a field releases in a teardown, where Svelte reads state
  // as it was before the update, so a reactive count loses the fields the
  // next tab mounted in the same update.
  let fields = 0
  let explicit = 0
  let asked = $state(false)
  let held = $state(false)
  let saved = $state(false)
  let timer: ReturnType<typeof setTimeout> | null = null
  const active = () => (declared() || asked) && !held
  return {
    get active() {
      return active()
    },
    get saved() {
      return saved
    },
    channel: {
      register: () => {
        fields += 1
        asked = true
      },
      release: () => {
        fields -= 1
        asked = fields > 0
      },
      hold: () => {
        explicit += 1
        held = true
      },
      drop: () => {
        explicit -= 1
        held = explicit > 0
      },
      flash: () => {
        if (!active()) return false
        saved = true
        if (timer) clearTimeout(timer)
        timer = setTimeout(() => (saved = false), ms)
        return true
      },
    },
    dispose: () => {
      if (timer) clearTimeout(timer)
    },
  }
}

/** Call in `SettingsPage`. Read `active` and `saved` off the returned object. */
export function provideAutosave(declared?: () => boolean, ms = 1800): { readonly active: boolean; readonly saved: boolean } {
  const cue = createAutosave(ms, declared)
  onDestroy(cue.dispose)
  setContext<AutosaveChannel | undefined>(KEY, cue.channel)
  return cue
}

/** Call in a surface that covers the page (a dialog). The header behind it is
 *  out of sight, so a control inside flashes its own row. */
export function isolateAutosave(): void {
  setContext<AutosaveChannel | undefined>(KEY, undefined)
}

const ONLY = Symbol('row')

/** Which rows are flashing "Saved", by the key each was flashed with. */
export function createRowSaved(ms = 2000): { flash: (key?: string) => void; saved: (key?: string) => boolean; dispose: () => void } {
  let on = $state<readonly (string | typeof ONLY)[]>([])
  const timers = new Map<string | typeof ONLY, ReturnType<typeof setTimeout>>()
  return {
    flash: (key) => {
      const k = key ?? ONLY
      if (!on.includes(k)) on = [...on, k]
      const running = timers.get(k)
      if (running) clearTimeout(running)
      timers.set(
        k,
        setTimeout(() => {
          timers.delete(k)
          on = on.filter((x) => x !== k)
        }, ms),
      )
    },
    saved: (key) => on.includes(key ?? ONLY),
    dispose: () => {
      for (const t of timers.values()) clearTimeout(t)
      timers.clear()
    },
  }
}

/** Run `on` while `when` holds (always, with no `when`) and `off` after. */
function whileMounted(when: (() => boolean) | undefined, on: () => void, off: () => void): void {
  if (when) {
    $effect(() => {
      if (!when()) return
      on()
      return off
    })
  } else {
    on()
    onDestroy(off)
  }
}

/** Call during init in a component whose controls save on change or blur.
 *  `flash(key?)` says a save landed: the header flashes when it shows the cue,
 *  and otherwise `saved(key?)` is true for two seconds, for the row's `saved`
 *  prop. One component with several such rows gives each its own key.
 *  `when` is for a component that only sometimes shows such controls (nothing
 *  set up yet, a viewer who cannot edit): it asks for the header cue while
 *  `when` returns true. */
export function useAutosave(when?: () => boolean): { flash: (key?: string) => void; saved: (key?: string) => boolean } {
  const channel = getContext<AutosaveChannel | undefined>(KEY)
  const rows = createRowSaved()
  onDestroy(rows.dispose)
  if (channel) whileMounted(when, channel.register, channel.release)
  return {
    flash: (key) => {
      if (!channel?.flash()) rows.flash(key)
    },
    saved: rows.saved,
  }
}

/** Call during init in a group that saves with a button. While it is mounted
 *  the page's header shows no autosave cue. `SettingsSaveBar` calls it, so a
 *  page only needs it for a Save that cannot be the bar. */
export function useExplicitSave(when?: () => boolean): void {
  const channel = getContext<AutosaveChannel | undefined>(KEY)
  if (channel) whileMounted(when, channel.hold, channel.drop)
}
