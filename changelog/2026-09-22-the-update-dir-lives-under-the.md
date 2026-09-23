- **The update dir lives under the state root, not a baked absolute path.**
  The image baked `TALARIA_UPDATE_DIR=/var/lib/talaria/update` — outside
  every deployment's bind (fleet VMs bind only their
  `/var/lib/talaria-<customer>` state dir), so the updater-owned compose
  and slot env files landed in the orchestrator's writable layer, gone on
  its next redeploy and invisible to a host-side inspection. The default
  is now `${TALARIA_STATE_DIR}/update`, the Dockerfile stamp is gone, and
  the variable is denylisted in slot renders so a stale baked value cannot
  propagate into the project it broke.
