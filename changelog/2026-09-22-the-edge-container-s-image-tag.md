- **The edge container's image tag never existed.** The pinned default
  `traefik:v3.6.7-alpine` was a tag nobody ever published — the 3.6 line
  tops at `v3.6.25` and carries no `-alpine` variants — so adoption's
  first fleet run brought green up and then silently never started the
  edge at all. The default is now the real `v3.6.25` (the plain tag is
  Alpine-based and carries the busybox `wget` the edge healthcheck needs).
