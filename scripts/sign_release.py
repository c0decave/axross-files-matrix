#!/usr/bin/env python3
"""Sign an axross-files-matrix release's SHA256SUMS on the maintainer's machine.

CI builds a release as a draft (.github/workflows/release.yml); the
updater (src/update.rs) takes no release without ``SHA256SUMS.sig``: the
raw 64-byte Ed25519 signature of the exact bytes of ``SHA256SUMS``, made
with a key whose public half src/update.rs pins (UPDATE_PUBLIC_KEY, in
TRUSTED_KEYS; all zeros - the placeholder - until the maintainer pins one). The
private key never goes to CI or into a repository (docs/RELEASING.md).

    sign_release.py keygen  --key KEY             a new key pair (the private
                                                  half to KEY, mode 600; the
                                                  public half printed, to pin)
    sign_release.py pubkey  --key KEY             the public half (to pin)
    sign_release.py sign    SHA256SUMS --key KEY  writes SHA256SUMS.sig
    sign_release.py release vX.Y.Z --key KEY [--publish]
        the draft's SHA256SUMS downloaded, every file of the draft checked
        against it, signed, SHA256SUMS.sig uploaded; --publish then makes the
        draft the latest release

A key whose public half is not pinned in src/update.rs is refused (a
release signed with it would be refused by every installed copy).
Needs Python 3.9+, the ``cryptography`` package and, for ``release``, the
GitHub CLI (``gh``) logged in with write access to the repository.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UPDATE_RS = ROOT / "src" / "update.rs"
REPO = "c0decave/axross-files-matrix"
SUMS = "SHA256SUMS"
SIG = "SHA256SUMS.sig"
_LINE = re.compile(r"([0-9a-f]{64}) [ *]([A-Za-z0-9][A-Za-z0-9._-]{0,199})")


def _crypto():
    try:
        from cryptography.hazmat.primitives import serialization
        from cryptography.hazmat.primitives.asymmetric.ed25519 import (
            Ed25519PrivateKey,
            Ed25519PublicKey,
        )
    except ImportError:
        sys.exit("sign_release.py needs the Python package cryptography (pip install cryptography)")
    return serialization, Ed25519PrivateKey, Ed25519PublicKey


def load_key(path: Path):
    serialization, private_type, _ = _crypto()
    st = path.stat()
    if os.name == "posix" and st.st_mode & 0o077:
        sys.exit(f"{path}: others may read the release key (chmod 600 it)")
    key = serialization.load_pem_private_key(path.read_bytes(), password=None)
    if not isinstance(key, private_type):
        sys.exit(f"{path} is not an Ed25519 private key")
    return key


def public_raw(key) -> bytes:
    serialization, _, _ = _crypto()
    return key.public_key().public_bytes(serialization.Encoding.Raw,
                                         serialization.PublicFormat.Raw)


def pinned_keys(source: Path = UPDATE_RS) -> list[bytes]:
    """The keys src/update.rs pins (UPDATE_PUBLIC_KEY's and TRUSTED_KEYS'
    32-byte arrays; the all-zero placeholder is no key)."""
    text = source.read_text(encoding="utf-8")
    keys = []
    for const in ("pub const UPDATE_PUBLIC_KEY", "pub const TRUSTED_KEYS"):
        if const not in text:
            continue
        block = text.split(const, 1)[1].split("];", 1)[0] + "]"
        for array in re.findall(r"\[([0-9a-fx,\s]+)\]", block):
            values = [int(v, 16) for v in re.findall(r"0x([0-9a-f]{2})", array)]
            if len(values) == 32 and any(values) and bytes(values) not in keys:
                keys.append(bytes(values))
    return keys


def sign(sums: bytes, key, trusted: list[bytes]) -> bytes:
    """The signature of *sums*, checked against the key's public half and
    refused when that is not one of *trusted*."""
    _, _, public_type = _crypto()
    raw = public_raw(key)
    if raw not in trusted:
        sys.exit("this key's public half is not pinned in src/update.rs (TRUSTED_KEYS): "
                 f"{base64.b64encode(raw).decode()}")
    for line in sums.decode("utf-8").splitlines():
        if line.strip() and not _LINE.fullmatch(line):
            sys.exit(f"{SUMS}: a line that is not '<sha256>  <name>': {line[:80]!r}")
    signature = key.sign(sums)
    public_type.from_public_bytes(raw).verify(signature, sums)
    assert len(signature) == 64
    return signature


def gh(*args: str, capture: bool = False) -> str:
    out = subprocess.run(["gh", *args], check=True, text=True,
                         stdout=subprocess.PIPE if capture else None)
    return out.stdout if capture else ""


def release(tag: str, key, trusted: list[bytes], repo: str, publish: bool,
            verify_files: bool) -> None:
    if not re.fullmatch(r"v[0-9]{1,6}\.[0-9]{1,6}\.[0-9]{1,6}", tag):
        sys.exit(f"{tag!r} is no vX.Y.Z tag")
    with tempfile.TemporaryDirectory(prefix="afm-sign-") as tmp:
        folder = Path(tmp)
        gh("release", "download", tag, "--repo", repo, "--pattern", SUMS, "--dir", tmp)
        sums = (folder / SUMS).read_bytes()
        listed = {}
        for line in sums.decode("utf-8").splitlines():
            match = _LINE.fullmatch(line)
            if match:
                listed[match.group(2)] = match.group(1)
        names = gh("release", "view", tag, "--repo", repo, "--json", "assets",
                   "--jq", ".assets[].name", capture=True).split()
        files = [n for n in names if n not in (SUMS, SIG)]
        missing = [n for n in files if n not in listed]
        if missing:
            sys.exit(f"files of the release without an entry in {SUMS}: {missing}")
        if verify_files:
            for name in files:
                gh("release", "download", tag, "--repo", repo, "--pattern", name,
                   "--dir", tmp, "--clobber")
                digest = hashlib.sha256((folder / name).read_bytes()).hexdigest()
                if digest != listed[name]:
                    sys.exit(f"{name}: its SHA-256 is not the one in {SUMS}")
                (folder / name).unlink()
                print(f"ok  {name}")
        (folder / SIG).write_bytes(sign(sums, key, trusted))
        gh("release", "upload", tag, str(folder / SIG), "--repo", repo, "--clobber")
        print(f"{SIG} uploaded to {repo} {tag}")
    if publish:
        gh("release", "edit", tag, "--repo", repo, "--draft=false", "--latest")
        print(f"{tag} published")


def keygen_refusal(key: Path) -> str | None:
    """Why a private key may not be written at *key* (None: it may): not
    inside a git work tree (it could be committed and published), not in a
    folder others may read or enter (RELEASING.md: a folder of mode 0700)."""
    folder = key.expanduser().absolute().parent
    probe = folder
    while True:
        if (probe / ".git").exists():
            return (f"{key}: inside the git work tree {probe} - a private key never goes "
                    "into a checkout; give a path outside every repository")
        if probe.parent == probe:
            break
        probe = probe.parent
    if not folder.is_dir():
        return f"{folder}: no such folder (make it with mode 0700 first)"
    if os.name == "posix" and folder.stat().st_mode & 0o077:
        return (f"{folder}: others may read or enter this folder (mode "
                f"{folder.stat().st_mode & 0o777:o}); keep the key in a folder of mode 0700")
    return None


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("keygen")
    p.add_argument("--key", type=Path, required=True)
    p = sub.add_parser("pubkey")
    p.add_argument("--key", type=Path, required=True)
    p = sub.add_parser("sign")
    p.add_argument("sums", type=Path)
    p.add_argument("--key", type=Path, required=True)
    p.add_argument("--out", type=Path)
    p.add_argument("--pinned-from", type=Path, default=UPDATE_RS,
                   help="the file whose TRUSTED_KEYS are pinned (default src/update.rs)")
    p = sub.add_parser("release")
    p.add_argument("tag")
    p.add_argument("--key", type=Path, required=True)
    p.add_argument("--repo", default=REPO)
    p.add_argument("--publish", action="store_true")
    p.add_argument("--no-verify-files", action="store_true",
                   help="sign without downloading every file to check its SHA-256")
    args = parser.parse_args(argv)
    if args.cmd == "keygen":
        refusal = keygen_refusal(args.key)
        if refusal:
            sys.exit(refusal)
        serialization, private_type, _ = _crypto()
        if args.key.exists():
            sys.exit(f"{args.key} exists: not overwritten")
        new = private_type.generate()
        pem = new.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                serialization.NoEncryption())
        fd = os.open(args.key, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as f:
            f.write(pem)
        raw = public_raw(new)
        print(f"{args.key} written (keep it off CI and out of every repository)")
        print("pin in src/update.rs as UPDATE_PUBLIC_KEY:")
        print("base64:", base64.b64encode(raw).decode())
        print("rust:  ", ", ".join(f"0x{b:02x}" for b in raw))
        return 0
    key = load_key(args.key)
    if args.cmd == "pubkey":
        raw = public_raw(key)
        print("base64:", base64.b64encode(raw).decode())
        print("rust:  ", ", ".join(f"0x{b:02x}" for b in raw))
        return 0
    if args.cmd == "sign":
        signature = sign(args.sums.read_bytes(), key, pinned_keys(args.pinned_from))
        out = args.out or args.sums.with_name(SIG)
        out.write_bytes(signature)
        print(f"{out} written")
        return 0
    release(args.tag, key, pinned_keys(), args.repo, args.publish, not args.no_verify_files)
    return 0


if __name__ == "__main__":
    sys.exit(main())
