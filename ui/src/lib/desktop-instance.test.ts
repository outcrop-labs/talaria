import { describe, expect, it } from 'vitest'
import { findCurrentInstance, instanceDisplayLabel, instanceHost } from './desktop-instance'

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
