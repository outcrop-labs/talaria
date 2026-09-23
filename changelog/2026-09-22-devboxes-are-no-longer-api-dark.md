- **Devboxes are no longer API-dark on arrival.** The box compose's
  `environment: PATH:` mirrored the node base image's PATH — but a compose
  override *replaces*, and the devbox image prepends `~/.cargo/bin`: with
  cargo off PATH, `talaria dev`'s api sidecar skipped itself (non-fatally, by
  design) and every fresh box served a UI whose every `/api/*` call failed.
  The override now mirrors the devbox image's PATH — cargo restored, plus the
  sbin directories the abbreviated form had also dropped — so the sidecar
  raises the Rust api on in-box loopback as intended.
