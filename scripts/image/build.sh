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
# Anything not passed as a flag is asked interactively: which storage holds
# the VM disks, which storage holds snippets (offering to enable it on
# `local` if none does — merging content types, never replacing them), and
# which SSH public key to inject — including generating and naming a new
# keypair when the host has none. With no terminal, the same choices become
# required flags. --dry-run prints the resolved plan and exits.
set -euo pipefail
cd "$(dirname "$0")/../.."

say()  { printf '\033[1;36m▸ %s\033[0m\n' "$*"; }
warn() { printf '\033[33m! %s\033[0m\n' "$*"; }
die()  { printf '\033[31m✗ %s\033[0m\n' "$*" >&2; exit 1; }
ask()  { local v=''; read -r -p "$1" v </dev/tty || true; printf '%s' "$v"; }

VMID=9000
STORAGE=
SNIP_STORAGE=
SNIP_DIR_OVERRIDE=
BRIDGE=vmbr0
CORES=4
MEMORY=16384
DISK=100G
IMAGE_URL=https://download.opensuse.org/tumbleweed/appliances/openSUSE-MicroOS.x86_64-ContainerHost-OpenStack-Cloud.qcow2
TIMEOUT=2700   # 45 min: transactional install + image pulls + a reboot
SSHKEYS=
DRY_RUN=0
while [ $# -gt 0 ]; do
  case "$1" in
    --vmid) VMID=$2; shift 2 ;;
    --storage) STORAGE=$2; shift 2 ;;
    --snippet-storage) SNIP_STORAGE=$2; shift 2 ;;
    --snippet-dir) SNIP_DIR_OVERRIDE=$2; shift 2 ;;
    --bridge) BRIDGE=$2; shift 2 ;;
    --cores) CORES=$2; shift 2 ;;
    --memory) MEMORY=$2; shift 2 ;;
    --disk) DISK=$2; shift 2 ;;
    --image-url) IMAGE_URL=$2; shift 2 ;;
    --timeout) TIMEOUT=$2; shift 2 ;;
    --sshkeys) SSHKEYS=$2; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    *) die "unknown option: $1" ;;
  esac
done

[ "$(id -u)" = 0 ] || die "run as root on the PVE host"
command -v qm >/dev/null || die "qm not found — this runs on a Proxmox host"
qm status "$VMID" >/dev/null 2>&1 && die "VM $VMID already exists — pick another --vmid"

# Prompts need a controlling terminal; without one, every unresolved choice
# becomes a hard error naming its flag (so scripted builds fail loudly at the
# top instead of halfway through a download).
interactive() { [ -t 0 ] && [ -e /dev/tty ]; }

# ── Resolve: VM disk storage ─────────────────────────────────────────────────
# List what PVE actually offers (storages that can hold images) instead of
# assuming local-lvm exists or is where you want a 100G template.
list_image_storages() {
  pvesm status --content images 2>/dev/null | tail -n +2 | awk 'NF && $3=="active"'
}
if [ -z "$STORAGE" ]; then
  mapfile -t rows < <(list_image_storages)
  [ ${#rows[@]} -gt 0 ] || die "no active image-capable storage found (pvesm status --content images)"
  if interactive; then
    printf 'VM disk storage:\n'
    for i in "${!rows[@]}"; do
      name=$(awk '{print $1}' <<<"${rows[$i]}")
      type=$(awk '{print $2}' <<<"${rows[$i]}")
      rest=$(awk '{ $1=$2=$3=""; sub(/^ +/,""); print }' <<<"${rows[$i]}")
      printf '  %d) %-16s %-8s %s\n' "$((i + 1))" "$name" "$type" "$rest"
    done
    default=$(awk '{print $1}' <<<"${rows[0]}")
    for i in "${!rows[@]}"; do
      [ "$(awk '{print $1}' <<<"${rows[$i]}")" = local-lvm ] && default=local-lvm
    done
    ans=$(ask "Which storage? [$default]: ")
    if [ -z "$ans" ]; then
      STORAGE=$default
    elif [[ $ans =~ ^[0-9]+$ ]] && [ "$ans" -ge 1 ] && [ "$ans" -le ${#rows[@]} ]; then
      STORAGE=$(awk '{print $1}' <<<"${rows[$((ans - 1))]}")
    else
      STORAGE=$ans
    fi
  else
    die "no terminal to prompt on — pass --storage (options: $(printf '%s ' $(list_image_storages | awk '{print $1}')))"
  fi
fi
pvesm status --content images 2>/dev/null | awk -v s="$STORAGE" '$1==s {found=1} END { exit !found }' \
  || die "storage '$STORAGE' does not exist or cannot hold images (pvesm status --content images)"

# ── Resolve: snippet storage + directory ─────────────────────────────────────
# cloud-init snippets live on a file-type storage. The PATH comes from
# /etc/pve/storage.cfg (pvesm status does not carry it), defaulting to the
# standard /var/lib/vz for `local` when the section has no explicit path.
# TALARIA_STORAGE_CFG exists so the parser can be exercised off-box.
STORAGE_CFG=${TALARIA_STORAGE_CFG:-/etc/pve/storage.cfg}
storagecfg_field() { # <storage> <field> — first value of that field in its section
  awk -v s="$1" -v f="$2" '
    /^[^ \t#]+: / { keep = ($2 == s) }
    keep && $1 == f { $1 = ""; sub(/^ +/, ""); print; exit }
  ' "$STORAGE_CFG"
}
snippet_dir_for() { # <storage>
  local path
  path=$(storagecfg_field "$1" path)
  [ -n "$path" ] || path=/var/lib/vz   # PVE's built-in default for `local`
  printf '%s/snippets' "$path"
}
enable_snippets_on_local() {
  [ -n "$(storagecfg_field local content)" ] || die "no 'local' storage to enable snippets on — pass --snippet-storage"
  # pvesm set --content REPLACES the content list, so merge by hand: enabling
  # snippets must not drop iso/backup/vztmpl from the storage.
  local cur new
  cur=$(storagecfg_field local content | tr -d ' ')
  case ",$cur," in *,snippets,*) ;; *) new="${cur:+$cur,}snippets"; pvesm set local --content "$new" ;; esac
}

if [ -z "$SNIP_STORAGE" ]; then
  mapfile -t snips < <(pvesm status --content snippets 2>/dev/null | tail -n +2 | awk 'NF && $3=="active" {print $1}')
  case ${#snips[@]} in
    1)
      SNIP_STORAGE=${snips[0]}
      say "snippet storage: $SNIP_STORAGE (only candidate)" ;;
    0)
      if interactive; then
        ans=$(ask "No storage accepts snippets yet. Enable on 'local'? [Y/n]: ")
        [[ ${ans,,} == n* ]] && die "enable snippets on a file storage, then re-run (e.g. pvesm set local --content snippets)"
        enable_snippets_on_local
        SNIP_STORAGE=local
      else
        die "no snippet storage (non-interactive) — run: pvesm set local --content snippets  (add to existing content, don't replace), then re-run with --snippet-storage"
      fi ;;
    *)
      if interactive; then
        printf 'Snippet storage:\n'
        for i in "${!snips[@]}"; do printf '  %d) %s\n' "$((i + 1))" "${snips[$i]}"; done
        ans=$(ask "Which storage? [1]: ")
        if [[ $ans =~ ^[0-9]+$ ]] && [ "$ans" -ge 1 ] && [ "$ans" -le ${#snips[@]} ]; then
          SNIP_STORAGE=${snips[$((ans - 1))]}
        elif [ -n "$ans" ]; then
          SNIP_STORAGE=$ans
        else
          SNIP_STORAGE=${snips[0]}
        fi
      else
        die "multiple snippet storages (non-interactive) — pass --snippet-storage (options: ${snips[*]})"
      fi ;;
  esac
fi
SNIP_DIR="${SNIP_DIR_OVERRIDE:-$(snippet_dir_for "$SNIP_STORAGE")}"
mkdir -p "$SNIP_DIR"   # the build writes the provisioning snippet here

# ── Resolve: SSH public key ──────────────────────────────────────────────────
# The key injected into the build VM and (per the printed clone recipe) into
# instances. None on the host yet? Offer to generate one, named by you.
if [ -z "$SSHKEYS" ]; then
  # TALARIA_KEY_HOME exists so key resolution can be exercised off-box.
  HOME_DIR=${TALARIA_KEY_HOME:-${HOME:-/root}}
  # conventional key names first, everything else alphabetical, empties dropped
  mapfile -t pubs < <(ls "$HOME_DIR"/.ssh/*.pub 2>/dev/null \
    | awk 'BEGIN{split("id_ed25519.pub id_ed25519_sk.pub id_ecdsa.pub id_rsa.pub",o," ");for(i in o)rank[o[i]]=i}
           {f=$0; sub(/.*\//,"",f); printf "%d\t%s\n", (f in rank ? rank[f] : 9), $0}' \
    | sort -n | cut -f2-)
  if interactive; then
    if [ ${#pubs[@]} -gt 0 ] && [ -n "${pubs[0]}" ]; then
      printf 'SSH public key to inject:\n'
      for i in "${!pubs[@]}"; do printf '  %d) %s\n' "$((i + 1))" "${pubs[$i]}"; done
      printf '  n) generate a new key\n  s) skip (console access only)\n'
      while :; do
        ans=$(ask "Which? [1]: ")
        if [ -z "$ans" ]; then SSHKEYS=${pubs[0]}; break; fi
        if [[ $ans =~ ^[0-9]+$ ]] && [ "$ans" -ge 1 ] && [ "$ans" -le ${#pubs[@]} ]; then
          SSHKEYS=${pubs[$((ans - 1))]}; break
        fi
        case $ans in
          n|N) SSHKEYS=GENERATE; break ;;
          s|S) SSHKEYS=SKIP; break ;;
          *) warn "answer 1-${#pubs[@]}, n, or s" ;;
        esac
      done
    else
      ans=$(ask "No SSH public keys on this host. Generate one? [Y/n]: ")
      [[ ${ans,,} == n* ]] || SSHKEYS=GENERATE
    fi
  elif [ ${#pubs[@]} -gt 0 ] && [ -n "${pubs[0]}" ]; then
    SSHKEYS=${pubs[0]}
    say "using $SSHKEYS (no terminal to prompt; pass --sshkeys to choose)"
  else
    warn "no --sshkeys, no public keys, no terminal — continuing without (build VM is console-only)"
  fi
fi
if [ "$SSHKEYS" = GENERATE ]; then
  name=$(ask "Name the key [talaria-image]: ")
  name=${name:-talaria-image}
  keyfile="$HOME_DIR/.ssh/$name"
  if [ -e "$keyfile" ]; then
    ans=$(ask "$keyfile exists — overwrite? [y/N]: ")
    [[ ${ans,,} == y* ]] || die "not overwriting $keyfile — pick another name or pass --sshkeys"
    rm -f "$keyfile" "$keyfile.pub"   # consent given; pre-remove so ssh-keygen can't re-ask
  fi
  if [ ! -d "$HOME_DIR/.ssh" ]; then
    mkdir -p "$HOME_DIR/.ssh"; chmod 700 "$HOME_DIR/.ssh"
  fi
  ssh-keygen -t ed25519 -N '' -C "$name" -f "$keyfile" >/dev/null
  # Under `sudo -E` the key lands in the invoker's home but is written as
  # root — hand ownership back so it's usable without sudo.
  if command -v getent >/dev/null && [ -n "${SUDO_USER:-}" ] \
     && [ "$(getent passwd "$SUDO_USER" | cut -d: -f6)" = "$HOME_DIR" ]; then
    chown "$SUDO_USER" "$keyfile" "$keyfile.pub" "$HOME_DIR/.ssh" 2>/dev/null || true
  fi
  SSHKEYS="$keyfile.pub"
  say "generated $keyfile"
  printf '  public key (add it anywhere this key should unlock):\n'
  sed 's/^/  /' "$SSHKEYS"
fi
[ "$SSHKEYS" = SKIP ] && SSHKEYS=
if [ -n "$SSHKEYS" ] && [ ! -f "$SSHKEYS" ]; then die "--sshkeys: $SSHKEYS is not a file"; fi

if [ "$DRY_RUN" = 1 ]; then
  echo
  printf '\033[1mPlan:\033[0m\n'
  echo "  VMID:            $VMID"
  echo "  disk storage:    $STORAGE"
  echo "  snippet storage: $SNIP_STORAGE → $SNIP_DIR"
  echo "  bridge:          $BRIDGE    cores: $CORES    memory: ${MEMORY}MiB    disk: $DISK"
  echo "  image:           $IMAGE_URL"
  echo "  ssh keys:        ${SSHKEYS:-(none — console only)}"
  echo "  build timeout:   $((TIMEOUT / 60)) min"
  echo "  dry run — nothing created."
  exit 0
fi

say "downloading base image"
IMG=$(basename "$IMAGE_URL")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fLo "$work/$IMG" "$IMAGE_URL"
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
state=
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
[ -z "$SSHKEYS" ] || echo "    qm set 301 --sshkeys $SSHKEYS"
echo "    qm set 301 --cicustom user=${SNIP_STORAGE}:snippets/tal-eu-1.yml   # optional env vars"
echo "    qm start 301"
echo
echo "  The env-var contract and the snippet shape: docs/SELF-HOSTING.md."
