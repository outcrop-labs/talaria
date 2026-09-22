- **Notifications reach a browser that's closed.** Web Push, end to end: the
  settings' desktop-notifications toggle now also files the browser as a push
  recipient (a per-device subscription — the push service's endpoint plus
  the key halves the payload is encrypted against), and any notification the
  single writer files with an in-app copy fans out to every subscribed
  browser of that person. The payload travels as RFC 8291 `aes128gcm`
  ciphertext under RFC 8292 VAPID proof — hand-assembled on `p256` rather
  than the OpenSSL-carrying `web-push` crate, keeping the runtime's
  no-OpenSSL invariant, and pinned to the RFCs' own test vectors. The
  instance's VAPID keypair is born once and its private half sealed under
  the instance secretbox. The service worker shows the OS notification only
  when no Talaria window is visible (the toast already covered the open
  case) and lands you on the notification's target when clicked; dead
  subscriptions (the service answered 404/410) prune themselves, and turning
  the toggle off retires the subscription at both ends. Classes routed
  **Email only** still reach mail alone.
