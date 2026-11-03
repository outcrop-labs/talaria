import { describe, expect, it } from 'vitest'
import type { ContextMenuItem } from '@/components/ui/context-menu.svelte'
import {
  findCurrentInstance,
  instanceDisplayLabel,
  instanceHost,
  reloadShortcut,
  switcherMenu,
} from './desktop-instance'

const a = { id: 'local-a', instanceId: 'uuid-a', label: 'Outcrop', url: 'https://talaria.example' }
const b = { id: 'local-b', instanceId: 'uuid-b', label: '  ', url: 'http://127.0.0.1:5302' }

describe('instanceDisplayLabel', () => {
  it('uses the stored label when it has a name', () => {
    expect(instanceDisplayLabel(a)).toBe('Outcrop')
  })

  it('falls back to the host when the company name was never set', () => {
    expect(instanceDisplayLabel(b)).toBe('127.0.0.1:5302')
    expect(instanceHost(b.url)).toBe('127.0.0.1:5302')
  })
})

describe('findCurrentInstance', () => {
  const list = [a, b]

  it('matches the beacon uuid when present', () => {
    expect(findCurrentInstance(list, 'uuid-b', 'https://other.example')?.id).toBe('local-b')
  })

  it('falls back to this webview origin when the beacon id is missing', () => {
    expect(findCurrentInstance(list, null, 'http://127.0.0.1:5302')?.id).toBe('local-b')
    expect(findCurrentInstance(list, '', 'https://talaria.example')?.id).toBe('local-a')
  })

  it('does not drop unnamed instances just because the beacon has no company name', () => {
    expect(findCurrentInstance(list, 'uuid-b', 'http://127.0.0.1:5302')?.url).toBe(b.url)
    expect(instanceDisplayLabel(findCurrentInstance(list, 'uuid-b', b.url)!)).toBe('127.0.0.1:5302')
  })
})

const MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)'
const LINUX = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko)'
const WINDOWS = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Edg/130.0'

describe('reloadShortcut', () => {
  it('names the key the shell takes on each OS', () => {
    expect(reloadShortcut(MAC)).toBe('⌘R')
    expect(reloadShortcut(LINUX)).toBe('Ctrl+R')
    expect(reloadShortcut(WINDOWS)).toBe('Ctrl+R')
  })
})

describe('switcherMenu', () => {
  const calls: string[] = []
  const act = {
    activate: (id: string) => calls.push(`activate ${id}`),
    reload: () => calls.push('reload'),
    manage: () => calls.push('manage'),
    quit: () => calls.push('quit'),
  }
  const labels = (entries: ReturnType<typeof switcherMenu>) =>
    entries.map((e) => (e === 'sep' ? '—' : e.label))
  const find = (entries: ReturnType<typeof switcherMenu>, label: string) =>
    entries.find((e): e is ContextMenuItem => e !== 'sep' && e.label === label)

  it('lists the instances, then Reload page, the launcher and Quit', () => {
    const menu = switcherMenu([a, b], 'local-b', LINUX, act)
    expect(labels(menu)).toEqual(['Outcrop', '127.0.0.1:5302', '—', 'Reload page', 'Add or manage instances', '—', 'Quit Talaria'])
    expect(find(menu, 'Outcrop')?.checked).toBe(false)
    expect(find(menu, '127.0.0.1:5302')?.checked).toBe(true)
  })

  it('offers Reload page with the shortcut for this OS, and it reloads', () => {
    calls.length = 0
    const linux = find(switcherMenu([a], 'local-a', LINUX, act), 'Reload page')
    expect(linux?.hint).toBe('Ctrl+R')
    linux?.onSelect?.()
    expect(calls).toEqual(['reload'])
    expect(find(switcherMenu([a], 'local-a', MAC, act), 'Reload page')?.hint).toBe('⌘R')
  })

  it('keeps Reload page when the shell lists no instances yet', () => {
    expect(labels(switcherMenu([], undefined, LINUX, act))).toEqual(['Reload page', 'Add or manage instances', '—', 'Quit Talaria'])
  })

  it('wires each row to its action', () => {
    calls.length = 0
    const menu = switcherMenu([a, b], undefined, LINUX, act)
    find(menu, 'Outcrop')?.onSelect?.()
    find(menu, 'Add or manage instances')?.onSelect?.()
    find(menu, 'Quit Talaria')?.onSelect?.()
    expect(calls).toEqual(['activate local-a', 'manage', 'quit'])
  })
})
