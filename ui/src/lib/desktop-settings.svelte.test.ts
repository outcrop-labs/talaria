// The shell window is shared: one instance's titlebar choice (or the
// launcher's) has to reach every other loaded instance when it is shown
// again, or the themed bar it drew at load stays up under the OS titlebar.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))

class FakeDocument extends EventTarget {
  hidden = false
}

const show = (doc: FakeDocument, hidden: boolean) => {
  doc.hidden = hidden
  doc.dispatchEvent(new Event('visibilitychange'))
}

describe('desktop settings inside the shell', () => {
  let doc: FakeDocument

  beforeEach(() => {
    vi.resetModules()
    invoke.mockReset()
    doc = new FakeDocument()
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
    vi.stubGlobal('document', doc)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('re-reads the titlebar mode each time this instance is shown again', async () => {
    const { loadDesktopSettings, desktopSettings } = await import('./desktop-settings.svelte')
    invoke.mockResolvedValue({ titlebar: 'themed' })
    await loadDesktopSettings()
    expect(desktopSettings().titlebar).toBe('themed')

    // Another instance switched the window to the OS titlebar while this one
    // was hidden.
    invoke.mockResolvedValue({ titlebar: 'os' })
    show(doc, true)
    expect(invoke).toHaveBeenCalledTimes(1)

    show(doc, false)
    await vi.waitFor(() => expect(desktopSettings().titlebar).toBe('os'))
    expect(invoke).toHaveBeenLastCalledWith('get_desktop_settings')
  })

  it('watches once, however often the titlebar asks to load', async () => {
    const { loadDesktopSettings } = await import('./desktop-settings.svelte')
    invoke.mockResolvedValue({ titlebar: 'themed' })
    await loadDesktopSettings()
    await loadDesktopSettings()
    await loadDesktopSettings()
    invoke.mockClear()

    show(doc, false)
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(1))
  })

  it('does nothing in a browser', async () => {
    vi.stubGlobal('window', {})
    const { loadDesktopSettings, desktopSettingsLoaded } = await import('./desktop-settings.svelte')
    await loadDesktopSettings()
    show(doc, false)
    expect(invoke).not.toHaveBeenCalled()
    expect(desktopSettingsLoaded()).toBe(false)
  })
})
