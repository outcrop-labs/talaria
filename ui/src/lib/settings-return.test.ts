import { describe, expect, it } from 'vitest'
import { APP_HOME, backToApp, rememberOutside } from './settings-return'

const visit = (stops: ReadonlyArray<[href: string, inside: boolean]>): string | null =>
  stops.reduce<string | null>((last, [href, inside]) => rememberOutside(last, href, inside), null)

describe('back to app', () => {
  it('goes home when the session opened inside the area', () => {
    expect(backToApp(visit([['/settings', true]]))).toBe(APP_HOME)
  })

  it('returns to the last address outside the area, query and hash included', () => {
    const last = visit([
      ['/home', false],
      ['/boards/eng?view=list#t-12', false],
      ['/manage', true],
    ])
    expect(backToApp(last)).toBe('/boards/eng?view=list#t-12')
  })

  it('keeps that address while the person moves between settings pages', () => {
    const last = visit([
      ['/comms/general', false],
      ['/manage', true],
      ['/models/roles', true],
      ['/settings/keys', true],
    ])
    expect(backToApp(last)).toBe('/comms/general')
  })

  it('follows the most recent visit outside', () => {
    const last = visit([
      ['/boards', false],
      ['/settings', true],
      ['/research', false],
      ['/admin', true],
    ])
    expect(backToApp(last)).toBe('/research')
  })

  it('never points at the page it is on', () => {
    const last = visit([
      ['/x/rates/manage', false],
      ['/x/rates/manage', true],
    ])
    expect(backToApp(last)).toBe(APP_HOME)
  })
})
