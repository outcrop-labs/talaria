// Client-side gate for a profile photo, run before any bytes leave the
// browser. It mirrors the server's claim check on `PUT /api/me
// { avatarUploadId }` (KTD8): PNG, JPEG, WebP or GIF, at most 5 MB. The
// server is still the authority; this only spares an upload the claim would
// refuse anyway.

export const PROFILE_PHOTO_MAX_BYTES = 5 * 1024 * 1024
export const PROFILE_PHOTO_TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'] as const
/** For the file input's `accept`. */
export const PROFILE_PHOTO_ACCEPT = PROFILE_PHOTO_TYPES.join(',')

/** A sentence describing why the file cannot be a profile photo, or null. */
export function validateProfilePhoto(file: { type: string; size: number }): string | null {
  if (!(PROFILE_PHOTO_TYPES as readonly string[]).includes(file.type))
    return 'Choose a PNG, JPEG, WebP, or GIF image.'
  if (file.size > PROFILE_PHOTO_MAX_BYTES) return 'That image is larger than 5 MB. Choose a smaller one.'
  return null
}

/** True when `picture` is the uploaded-avatar route (`/api/users/{id}/avatar`)
 *  rather than a Google picture — only an upload can be removed. */
export function isUploadedAvatar(picture: string | null | undefined): boolean {
  return !!picture && /^\/api\/users\/[^/]+\/avatar(\?|$)/.test(picture)
}
