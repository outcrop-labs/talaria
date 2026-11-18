#!/usr/bin/env python3
"""Build Tauri's static updater manifest (latest.json) from signed assets.

The updater reads GitHub's /releases/latest/download/latest.json — that URL
only ever serves the latest STABLE release — so this script is what a stable
tag's attach job writes into the release. Every platform entry must be
complete (url + signature contents); Tauri rejects the whole file if one
row is short.

Every signature must come from the key whose public half this build ships
(tauri.conf.json's updater pubkey). An install checks a payload against the
pubkey it was built with, so a feed signed by any other key strands every
copy built from this commit: Update fails on the signature for good. Only
the key ID is compared here; the cryptographic check is the plugin's.
"""

from __future__ import annotations

import base64
import binascii
import glob
import json
import os
import sys
from datetime import datetime, timezone


def find_one(directory: str, *patterns: str) -> str | None:
    hits: list[str] = []
    for pattern in patterns:
        hits.extend(glob.glob(os.path.join(directory, pattern)))
    return hits[0] if hits else None


CONF = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src-tauri", "tauri.conf.json")


def key_id(box: str) -> str:
    """The key ID of a Tauri-encoded minisign box (a .sig file, or the pubkey).

    Tauri base64-encodes the whole minisign text. Its second line is
    base64 of a 2-byte algorithm, the 8-byte key ID, then the key or
    signature. minisign prints the ID as a little-endian u64, so the bytes
    are reversed to match the `minisign public key: <ID>` comment.
    """
    try:
        raw = base64.b64decode(base64.b64decode(box).decode().splitlines()[1])
    except (binascii.Error, UnicodeDecodeError, IndexError) as err:
        raise SystemExit(f"not a minisign box: {err}") from err
    if len(raw) < 10:
        raise SystemExit("not a minisign box: too short for a key ID")
    return raw[2:10][::-1].hex().upper()


def shipped_key_id(conf: str = CONF) -> str:
    with open(conf, encoding="utf-8") as fh:
        return key_id(json.load(fh)["plugins"]["updater"]["pubkey"])


def entry(path: str | None, tag: str, repo: str, want: str) -> dict[str, str] | None:
    if not path:
        return None
    sig_path = path + ".sig"
    if not os.path.isfile(sig_path):
        return None
    with open(sig_path, encoding="utf-8") as fh:
        signature = fh.read().strip()
    if not signature:
        return None
    name = os.path.basename(path)
    got = key_id(signature)
    if got != want:
        raise SystemExit(
            f"{name}.sig is signed by key {got}, but this build ships the updater pubkey "
            f"{want} (tauri.conf.json): every install would refuse it. Sign with the key "
            "whose public half is in tauri.conf.json, or ship the new pubkey first."
        )
    return {
        "url": f"https://github.com/{repo}/releases/download/{tag}/{name}",
        "signature": signature,
    }


def build(version: str, tag: str, repo: str, assets: str, want: str | None = None) -> dict:
    want = want or shipped_key_id()
    platforms: dict[str, dict[str, str]] = {}
    linux = entry(
        find_one(assets, "Talaria_*_amd64.AppImage", "Talaria_*_x86_64.AppImage"),
        tag,
        repo,
        want,
    )
    if linux:
        platforms["linux-x86_64"] = linux
    darwin = entry(find_one(assets, "Talaria-*-universal.app.tar.gz"), tag, repo, want)
    if darwin:
        # Universal binary: every Mac arch downloads the same tarball.
        platforms["darwin-x86_64"] = darwin
        platforms["darwin-aarch64"] = darwin
        platforms["darwin-universal"] = darwin
    windows = entry(find_one(assets, "Talaria_*_x64-setup.exe"), tag, repo, want)
    if windows:
        platforms["windows-x86_64"] = windows
    if not platforms:
        raise SystemExit(f"no signed updater artifacts in {assets}")
    return {
        "version": version,
        "notes": f"Talaria Desktop {version}. See CHANGELOG.md.",
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }


def main(argv: list[str]) -> None:
    if argv == ["--self-test"]:
        return _self_test()
    if len(argv) != 4:
        raise SystemExit(f"usage: {sys.argv[0]} VERSION TAG REPO ASSETS_DIR")
    version, tag, repo, assets = argv
    doc = build(version, tag, repo, assets)
    out = os.path.join(assets, "latest.json")
    with open(out, "w", encoding="utf-8") as fh:
        json.dump(doc, fh, indent=2)
        fh.write("\n")
    print(f"wrote {out} for {', '.join(doc['platforms'])}")


def _box(kind: bytes, kid: str, comment: str) -> str:
    """A Tauri-encoded minisign box with key ID `kid` (signature bytes zeroed)."""
    raw = kind + bytes.fromhex(kid)[::-1] + bytes(64 if kind == b"ED" else 32)
    text = f"untrusted comment: {comment}\n{base64.b64encode(raw).decode()}\n"
    return base64.b64encode(text.encode()).decode()


def _self_test() -> None:
    import tempfile

    # The shipped pubkey parses, and its ID is the one in its own comment.
    pub = base64.b64decode(json.load(open(CONF, encoding="utf-8"))["plugins"]["updater"]["pubkey"]).decode()
    assert pub.splitlines()[0].endswith(shipped_key_id()), pub.splitlines()[0]

    want, other = "183E3397DE42EB98", "A7E4E3180ED0F727"
    sigs = {name: _box(b"ED", want, f"signature {name}") for name in ("linux", "mac", "win")}
    with tempfile.TemporaryDirectory() as tmp:
        app = os.path.join(tmp, "Talaria_1.2.3_amd64.AppImage")
        open(app, "wb").close()
        with open(app + ".sig", "w", encoding="utf-8") as fh:
            fh.write(sigs["linux"] + "\n")
        mac = os.path.join(tmp, "Talaria-1.2.3-universal.app.tar.gz")
        open(mac, "wb").close()
        with open(mac + ".sig", "w", encoding="utf-8") as fh:
            fh.write(sigs["mac"] + "\n")
        win = os.path.join(tmp, "Talaria_1.2.3_x64-setup.exe")
        open(win, "wb").close()
        with open(win + ".sig", "w", encoding="utf-8") as fh:
            fh.write(sigs["win"] + "\n")
        doc = build("1.2.3", "v1.2.3", "outcrop-labs/talaria", tmp, want)
        assert doc["version"] == "1.2.3"
        assert doc["platforms"]["linux-x86_64"]["signature"] == sigs["linux"]
        assert doc["platforms"]["darwin-aarch64"]["url"].endswith(
            "Talaria-1.2.3-universal.app.tar.gz"
        )
        assert (
            doc["platforms"]["darwin-x86_64"]
            == doc["platforms"]["darwin-universal"]
            == doc["platforms"]["darwin-aarch64"]
        )
        assert doc["platforms"]["windows-x86_64"]["signature"] == sigs["win"]
        # One payload signed by another key refuses the whole feed.
        with open(mac + ".sig", "w", encoding="utf-8") as fh:
            fh.write(_box(b"ED", other, "signature from the old key") + "\n")
        try:
            build("1.2.3", "v1.2.3", "outcrop-labs/talaria", tmp, want)
        except SystemExit as err:
            assert other in str(err) and want in str(err), err
        else:
            raise SystemExit("expected a signature by another key to fail")
        with open(mac + ".sig", "w", encoding="utf-8") as fh:
            fh.write("SIG-MAC\n")
        try:
            build("1.2.3", "v1.2.3", "outcrop-labs/talaria", tmp, want)
        except SystemExit as err:
            assert "not a minisign box" in str(err), err
        else:
            raise SystemExit("expected a malformed signature to fail")
        empty = tempfile.mkdtemp()
        try:
            build("1.2.3", "v1.2.3", "outcrop-labs/talaria", empty, want)
        except SystemExit:
            pass
        else:
            raise SystemExit("expected empty assets to fail")
    print("self-test ok")


if __name__ == "__main__":
    main(sys.argv[1:])
