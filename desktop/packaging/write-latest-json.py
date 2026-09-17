#!/usr/bin/env python3
"""Build Tauri's static updater manifest (latest.json) from signed assets.

The updater reads GitHub's /releases/latest/download/latest.json — that URL
only ever serves the latest STABLE release — so this script is what a stable
tag's attach job writes into the release. Every platform entry must be
complete (url + signature contents); Tauri rejects the whole file if one
row is short.
"""

from __future__ import annotations

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


def entry(path: str | None, tag: str, repo: str) -> dict[str, str] | None:
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
    return {
        "url": f"https://github.com/{repo}/releases/download/{tag}/{name}",
        "signature": signature,
    }


def build(version: str, tag: str, repo: str, assets: str) -> dict:
    platforms: dict[str, dict[str, str]] = {}
    linux = entry(
        find_one(assets, "Talaria_*_amd64.AppImage", "Talaria_*_x86_64.AppImage"),
        tag,
        repo,
    )
    if linux:
        platforms["linux-x86_64"] = linux
    darwin = entry(find_one(assets, "Talaria-*-universal.app.tar.gz"), tag, repo)
    if darwin:
        # Universal binary: every Mac arch downloads the same tarball.
        platforms["darwin-x86_64"] = darwin
        platforms["darwin-aarch64"] = darwin
        platforms["darwin-universal"] = darwin
    windows = entry(find_one(assets, "Talaria_*_x64-setup.exe"), tag, repo)
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


def _self_test() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        app = os.path.join(tmp, "Talaria_1.2.3_amd64.AppImage")
        open(app, "wb").close()
        with open(app + ".sig", "w", encoding="utf-8") as fh:
            fh.write("SIG-LINUX\n")
        mac = os.path.join(tmp, "Talaria-1.2.3-universal.app.tar.gz")
        open(mac, "wb").close()
        with open(mac + ".sig", "w", encoding="utf-8") as fh:
            fh.write("SIG-MAC\n")
        win = os.path.join(tmp, "Talaria_1.2.3_x64-setup.exe")
        open(win, "wb").close()
        with open(win + ".sig", "w", encoding="utf-8") as fh:
            fh.write("SIG-WIN\n")
        doc = build("1.2.3", "v1.2.3", "outcrop-labs/talaria", tmp)
        assert doc["version"] == "1.2.3"
        assert doc["platforms"]["linux-x86_64"]["signature"] == "SIG-LINUX"
        assert doc["platforms"]["darwin-aarch64"]["url"].endswith(
            "Talaria-1.2.3-universal.app.tar.gz"
        )
        assert (
            doc["platforms"]["darwin-x86_64"]
            == doc["platforms"]["darwin-universal"]
            == doc["platforms"]["darwin-aarch64"]
        )
        assert doc["platforms"]["windows-x86_64"]["signature"] == "SIG-WIN"
        empty = tempfile.mkdtemp()
        try:
            build("1.2.3", "v1.2.3", "outcrop-labs/talaria", empty)
        except SystemExit:
            pass
        else:
            raise SystemExit("expected empty assets to fail")
    print("self-test ok")


if __name__ == "__main__":
    main(sys.argv[1:])
