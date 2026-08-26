#!/usr/bin/env bash
# Build the Talaria golden image on a Proxmox host.
#
#   git clone https://github.com/outcrop-labs/talaria   # on the PVE host
#   sudo ./scripts/image/build.sh
#
# What it makes: a VM from the official openSUSE MicroOS ContainerHost cloud
# image, provisioned (docker + compose, tailscale, firewalld rules, bun, the
# first-boot installer) via a cloud-init snippet that embeds
# scripts/image/{provision.sh,firstboot.sh} and the two units — then a
# Proxmox TEMPLATE. The image carries no Talaria checkout; every clone
# installs current Talaria on its own first boot (docs/SELF-HOSTING.md).
#
# One-time PVE prep this script does NOT do for you (it won't mutate storage
# config unbidden): the snippet storage must exist —
#   pvesm set local --content snippets
set -euo pipefail
cd "$(dirname "$0")/../.."

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
warn() { printf '\033[33m! %s\033[0m\n' "$*"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

VMID=9000
STORAGE=local-lvm
SNIP_STORAGE=local
BRIDGE=vmbr0
CORES=4
MEMORY=16384
DISK=100G
IMAGE_URL=https://download.opensuse.org/tumbleweed/appliances/openSUSE-MicroOS.x86_64-ContainerHost-OpenStack-Cloud.qcow2
TIMEOUT=2700   # 45 min: transactional install + image pulls + a reboot
SSHKEYS="${SSHKEYS:-}"
while [ $# -gt 0 ]; do
  case "$1" in
    --vmid) VMID=$2; shift 2 ;;
    --storage) STORAGE=$2; shift 2 ;;
    --snippet-storage) SNIP_STORAGE=$2; shift 2 ;;
    --bridge) BRIDGE=$2; shift 2 ;;
    --cores) CORES=$2; shift 2 ;;
    --memory) MEMORY=$2; shift 2 ;;
    --disk) DISK=$2; shift 2 ;;
    --image-url) IMAGE_URL=$2; shift 2 ;;
    --timeout) TIMEOUT=$2; shift 2 ;;
    --sshkeys) SSHKEYS=$2; shift 2 ;;
    *) die "unknown option: $1" ;;
  esac
done

[ "$(id -u)" = 0 ] || die "run as root on the PVE host"
command -v qm >/dev/null || die "qm not found — this runs on a Proxmox host"
qm status "$VMID" >/dev/null 2>&1 && die "VM $VMID already exists — pick another --vmid"

# The snippet directory. Proxmox keeps snippets at <storage-path>/snippets on
# dir-type storages; only 'local' is handled by default, override with
# --snippet-storage (after enabling snippets content on it).
SNIP_DIR="/var/lib/vz/snippets"
[ "$SNIP_STORAGE" = local ] || SNIP_DIR=$(pvesm status | awk -v s="$SNIP_STORAGE" '$1==s {print $2}')/snippets
[ -d "$SNIP_DIR" ] || die "snippet dir $SNIP_DIR missing — run: pvesm set $SNIP_STORAGE --content snippets"

say "downloading base image"
IMG=$(basename "$IMAGE_URL")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
[ -f "$work/$IMG" ] || curl -fLo "$work/$IMG" "$IMAGE_URL"
curl -fLo "$work/$IMG.sha256" "$IMAGE_URL.sha256"
(cd "$work" && sha256sum -c "$IMG.sha256") || die "image checksum mismatch"

say "generating the provisioning snippet"
# Embed the repo's own files via write_files (YAML literal blocks — one
# write_files key, many entries; repeating the key per file would make YAML
# keep only the last). The runcmd enable --now is load-bearing: enable-only
# would wait for the next boot that never comes, and this script would poll a
# VM that never stops.
SNIPPET="talaria-image-build-$VMID.yml"
{
  echo '#cloud-config'
  echo 'write_files:'
  emit() { # emit <repo file> <guest path> <mode>
    printf '  - path: %s\n    permissions: %s\n    content: |\n' "$2" "'$3'"
    sed 's/^/      /' "$1"
  }
  emit scripts/image/provision.sh              /etc/talaria/provision.sh                          0755
  emit scripts/image/firstboot.sh              /etc/talaria/firstboot.sh                          0755
  emit scripts/image/talaria-provision.service /etc/systemd/system/talaria-provision.service      0644
  emit scripts/image/talaria-firstboot.service /etc/systemd/system/talaria-firstboot.service      0644
  echo 'runcmd:'
  echo '  - [ systemctl, daemon-reload ]'
  echo '  - [ systemctl, enable, --now, talaria-provision ]'
} > "$SNIP_DIR/$SNIPPET"

say "creating build VM $VMID"
# pre-enrolled-keys=0: no secure-boot signing surprises in a golden image.
qm create "$VMID" --name talaria-image --ostype l26 --machine q35 --bios ovmf \
  --efidisk0 "${STORAGE}:1,efitype=4m,pre-enrolled-keys=0" \
  --cores "$CORES" --memory "$MEMORY" --net0 "virtio,bridge=${BRIDGE}" \
  --agent enabled=1 --serial0 socket --vga serial0
qm importdisk "$VMID" "$work/$IMG" "$STORAGE" >/dev/null
# importdisk leaves the disk unattached as unused0 — take whatever it is.
DISK_VOL=$(qm config "$VMID" | awk '/^unused0:/ {print $2}')
[ -n "$DISK_VOL" ] || die "importdisk produced no unused disk"
qm set "$VMID" --scsihw virtio-scsi-single --scsi0 "${DISK_VOL},discard=on"
qm resize "$VMID" scsi0 "$DISK"
# The cloudinit drive must exist even though the user-data comes from cicustom
# (cicustom swaps the CONTENT, the drive carries ciuser/sshkeys/ipconfig).
qm set "$VMID" --ide2 "${STORAGE}:cloudinit" --ciuser talaria --ipconfig0 ip=dhcp
[ -z "$SSHKEYS" ] || qm set "$VMID" --sshkeys "$SSHKEYS"
qm set "$VMID" --cicustom "user=${SNIP_STORAGE}:snippets/${SNIPPET}"
qm set "$VMID" --boot order=scsi0

say "starting — provisioning runs inside (serial: qm terminal $VMID)"
qm start "$VMID"

say "waiting for the VM to power itself off (template signal)"
elapsed=0
while [ "$elapsed" -lt "$TIMEOUT" ]; do
  state=$(qm status "$VMID" 2>/dev/null | awk '{print $2}')
  [ "$state" = stopped ] && break
  sleep 15
  elapsed=$((elapsed + 15))
done
if [ "$state" != stopped ]; then
  warn "VM never stopped within $((TIMEOUT / 60)) min — provisioning is stuck or failed."
  warn "Inspect:  qm terminal $VMID        (then Ctrl+O to detach)"
  warn "The VM is left RUNNING for inspection; destroy with:  qm destroy $VMID --purge"
  exit 1
fi

say "templating"
qm template "$VMID"

echo
printf '\033[1;32mTemplate ready: %s (vmid %s).\033[0m\n\n' talaria-image "$VMID"
echo "  Spin an instance:"
echo "    qm clone $VMID 301 --name tal-eu-1 --full"
echo "    qm set 301 --sshkeys ~/.ssh/id_ed25519.pub"
echo "    qm set 301 --cicustom user=${SNIP_STORAGE}:snippets/tal-eu-1.yml   # optional env vars"
echo "    qm start 301"
echo
echo "  The env-var contract and the snippet shape: docs/SELF-HOSTING.md."
