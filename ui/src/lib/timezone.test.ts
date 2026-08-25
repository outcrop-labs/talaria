import { describe, expect, it } from 'vitest'
import { detectedZone, isValidTimeZone, supportedTimeZones, zoneNowLabel } from '@/lib/timezone'

// The zone helpers both sides share. The pins that matter:
//   - validity is "Intl can resolve it", not string shape — 'Denver' looks
//     like a zone and is not one, and a runtime that knows only a subset of
//     IANA still gets a truthful answer for the names it carries;
//   - detection NEVER throws — adoption runs on app boot, and a missing or
//     misbehaving Intl must degrade to null, not crash the layout.

describe('isValidTimeZone', () => {
  it('accepts real IANA names and rejects lookalikes', () => {
    expect(isValidTimeZone('America/Denver')).toBe(true)
    expect(isValidTimeZone('Asia/Kolkata')).toBe(true)
    expect(isValidTimeZone('UTC')).toBe(true)
    // 'Denver' is not an IANA name; empty and garbage are not either.
    expect(isValidTimeZone('Denver')).toBe(false)
    expect(isValidTimeZone('')).toBe(false)
    expect(isValidTimeZone('not/a zone!!')).toBe(false)
  })
})

describe('detectedZone', () => {
  it('answers a valid zone or null, and never throws', () => {
    const zone = detectedZone()
    expect(zone === null || (typeof zone === 'string' && isValidTimeZone(zone))).toBe(true)
  })
})

describe('supportedTimeZones', () => {
  it('lists zones that include the detected one wherever the API exists', () => {
    const zones = supportedTimeZones()
    expect(Array.isArray(zones)).toBe(true)
    for (const z of zones) expect(typeof z).toBe('string')
    const detected = detectedZone()
    if (zones.length > 0 && detected) {
      // Where supportedValuesOf exists it is the authority, so it must know
      // the zone this same runtime detects.
      expect(zones).toContain(detected)
    }
  })

  it('offers UTC even when the runtime leaves it off the supported list', () => {
    // CI is the canonical case: no TZ set, so detection answers UTC while
    // supportedValuesOf lists only geographic zones. The union must restore
    // the one zone every runtime can honor, in the list's alphabetical order.
    const intl = Intl as unknown as { supportedValuesOf?: (k: string) => string[] }
    const original = intl.supportedValuesOf
    try {
      intl.supportedValuesOf = () => ['America/Denver', 'Asia/Kolkata']
      expect(supportedTimeZones()).toEqual(['America/Denver', 'Asia/Kolkata', 'UTC'])
    } finally {
      // Restore the platform's own answer, not a saved clone of it.
      if (original) intl.supportedValuesOf = original
      else delete intl.supportedValuesOf
    }
  })
})

describe('zoneNowLabel', () => {
  it('renders a time in a real zone and nothing in a bogus one', () => {
    expect(zoneNowLabel('America/Denver')).toMatch(/\d/)
    expect(zoneNowLabel('bogus/zone')).toBe('')
  })
})
