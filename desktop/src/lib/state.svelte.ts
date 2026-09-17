/** The launcher's whole state. Rust owns persistence and geometry; this owns
 * what the sidebar shows and which instance is highlighted. */

import {
  activateInstance,
  addInstance,
  errorText,
  listInstances,
  removeInstance,
  showWelcome,
  type Instance,
} from './instances'

export const shell = $state({
  instances: [] as Instance[],
  activeId: null as string | null,
  loaded: false,
  error: '',
})

export async function open(id: string) {
  shell.error = ''
  try {
    await activateInstance(id)
    shell.activeId = id
  } catch (e) {
    shell.error = errorText(e)
  }
}

export async function add(url: string): Promise<boolean> {
  shell.error = ''
  try {
    const instance = await addInstance(url)
    shell.instances.push(instance)
    await open(instance.id)
    return true
  } catch (e) {
    shell.error = errorText(e)
    return false
  }
}

export async function remove(id: string) {
  shell.error = ''
  try {
    shell.instances = await removeInstance(id)
    if (shell.activeId === id) {
      shell.activeId = null
      await welcome()
    }
  } catch (e) {
    shell.error = errorText(e)
  }
}

export async function welcome() {
  shell.error = ''
  try {
    await showWelcome()
  } catch (e) {
    shell.error = errorText(e)
  }
}

/** Boot: load the registry and reopen the most recently opened instance, if
 * any — the desktop app remembers where you were. */
export async function boot() {
  try {
    shell.instances = await listInstances()
  } catch (e) {
    shell.error = errorText(e)
  }
  const last = [...shell.instances].sort(
    (a, b) => (b.lastOpenedAt ?? 0) - (a.lastOpenedAt ?? 0),
  )[0]
  if (last) await open(last.id)
  shell.loaded = true
}
