- **The inherit path's edge-boot helper was born blind to docker.** The
  helper runs the app image as its own non-root user, and its `docker run`
  carried no `--group-add` — so the 0660 root:docker socket denied its
  every call. The first shape's `docker inspect … 2>/dev/null | grep -q
  true` read that denial as *the old container already stopped*, the wait
  skipped, the compose refused, and the helper exited before the port ever
  freed: the old container stopped itself for the one-time cut, the edge
  never rose, and nothing served the port (a client VPS took exactly this
  outage). The helper now carries the same host group ids as the app
  container, and its script treats an unreachable daemon as a loud failure
  with a breadcrumb — `edge-boot.log` in the update dir — instead of a
  confident answer it never had.
