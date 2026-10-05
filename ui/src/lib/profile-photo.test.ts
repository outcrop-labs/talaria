import { describe, expect, it } from 'vitest'
import { PROFILE_PHOTO_MAX_BYTES, isUploadedAvatar, validateProfilePhoto } from './profile-photo'

const MB = 1024 * 1024

describe('validateProfilePhoto', () => {
  it('rejects an image over 5 MB with a sentence', () => {
    const err = validateProfilePhoto({ type: 'image/png', size: 6 * MB })
    expect(err).toMatch(/5 MB/)
    expect(err).toMatch(/\.$/)
  })

  it('rejects a PDF', () => {
    expect(validateProfilePhoto({ type: 'application/pdf', size: 100_000 })).toMatch(/PNG, JPEG, WebP, or GIF/)
  })

  it('rejects an image type the server will not claim (svg)', () => {
    expect(validateProfilePhoto({ type: 'image/svg+xml', size: 1000 })).not.toBeNull()
  })

  it('accepts a 1 MB WebP', () => {
    expect(validateProfilePhoto({ type: 'image/webp', size: 1 * MB })).toBeNull()
  })

  it('accepts exactly 5 MB', () => {
    expect(PROFILE_PHOTO_MAX_BYTES).toBe(5 * MB)
    expect(validateProfilePhoto({ type: 'image/jpeg', size: 5 * MB })).toBeNull()
  })

  it('accepts png and gif', () => {
    expect(validateProfilePhoto({ type: 'image/png', size: 10 })).toBeNull()
    expect(validateProfilePhoto({ type: 'image/gif', size: 10 })).toBeNull()
  })
})

describe('isUploadedAvatar', () => {
  it('is true for the avatar route the server returns for an upload', () => {
    expect(isUploadedAvatar('/api/users/0b7c/avatar?v=1a2b3c')).toBe(true)
  })
  it('is false for a Google picture or none', () => {
    expect(isUploadedAvatar('https://lh3.googleusercontent.com/a/xyz')).toBe(false)
    expect(isUploadedAvatar(null)).toBe(false)
    expect(isUploadedAvatar(undefined)).toBe(false)
  })
})
