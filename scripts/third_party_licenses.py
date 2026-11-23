"""The licences of what axross-files-matrix is built from, for one target, from
Cargo.lock (through `cargo metadata`): every crate linked into the program
(normal dependencies; not build scripts' or tests'), its licence, and the
licence texts its package carries - the files at its top and in its folders
two deep named LICENSE, COPYING, NOTICE, OFL, UFL ... (egui's fonts: the
OFL, the Ubuntu Font Licence and the icon fonts' licences in
epaint_default_fonts/fonts). A crate that carries no text gets the standard
text of its licence from scripts/licenses/.

    python3 scripts/third_party_licenses.py --target x86_64-unknown-linux-gnu --output THIRD_PARTY_LICENSES.md
    python3 scripts/third_party_licenses.py --target x86_64-unknown-linux-gnu --check THIRD_PARTY_LICENSES.md

--check exits 1 when the file is not what Cargo.lock gives now (a crate
added, updated or removed without the file made again).
"""

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
STANDARD = HERE / "licenses"
NAMES = re.compile(r"(licen[cs]e|copying|notice|copyright|unlicense|^ofl|^ufl)", re.I)
SKIP_DIRS = {"src", "tests", "benches", "examples", "target", ".git", ".github"}


def metadata(target, offline):
    args = ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target]
    if offline:
        args.append("--offline")
    out = subprocess.run(args, cwd=ROOT, check=True, capture_output=True, text=True).stdout
    return json.loads(out)


def linked(meta):
    """The packages linked into the program: from the root, normal edges."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, todo = set(), [root]
    while todo:
        pid = todo.pop()
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                todo.append(dep["pkg"])
    packages = {p["id"]: p for p in meta["packages"]}
    return sorted((packages[i] for i in seen), key=lambda p: (p["name"], p["version"]))


def texts(package):
    """(file, text) of the licence files the package carries."""
    top = Path(package["manifest_path"]).parent
    found = []
    for path in sorted(top.rglob("*")):
        rel = path.relative_to(top)
        if not path.is_file() or len(rel.parts) > 3:
            continue
        if any(part in SKIP_DIRS for part in rel.parts[:-1]):
            continue
        name = path.name
        fonts = "fonts" in rel.parts[:-1] and name.endswith(".txt")
        if not (NAMES.search(name) or fonts):
            continue
        if path.suffix in {".rs", ".toml", ".json", ".html", ".py", ".sh"}:
            continue
        try:
            # (form feeds - page breaks of old licence texts - left out:
            # no invisible control characters in the file)
            text = path.read_text(encoding="utf-8", errors="replace").replace("\f", "")
        except OSError:
            continue
        if text.strip():
            found.append((str(rel), text.strip() + "\n"))
    if package.get("license_file"):
        own = top / package["license_file"]
        if own.is_file() and all(f != package["license_file"] for f, _ in found):
            found.append((package["license_file"], own.read_text(encoding="utf-8", errors="replace").strip() + "\n"))
    return found


# (how a carried text is known to be of a licence: one whose text is not
# carried gets the standard one)
MARKS = {
    "MIT": "Permission is hereby granted",
    "Apache-2.0": "Apache License",
    "BSL-1.0": "Boost Software License",
}


def standard(expression, found):
    """The standard texts of the licences named in an SPDX expression that
    the package's own files do not carry."""
    out = []
    for spdx in sorted(set(re.findall(r"[A-Za-z0-9.\-+]+", expression or "")) - {"AND", "OR", "WITH"}):
        path = STANDARD / f"{spdx}.txt"
        mark = MARKS.get(spdx)
        if path.is_file() and not (mark and any(mark in t for _, t in found)):
            out.append((f"standard text: {spdx}", path.read_text(encoding="utf-8").strip() + "\n"))
    return out


def render(target, packages):
    lines = [
        "# Third-party licences",
        "",
        f"What axross-files-matrix is built from for `{target}`, from Cargo.lock: each crate,",
        "its licence and the licence texts it carries (made by",
        "`scripts/third_party_licenses.py` - not edited by hand). axross-files-matrix itself:",
        "LICENSE (Apache-2.0).",
        "",
        "| Crate | Version | Licence |",
        "|---|---|---|",
    ]
    groups = {}
    missing = []
    for p in packages:
        lic = p.get("license") or ("see " + p["license_file"] if p.get("license_file") else "?")
        lines.append(f"| {p['name']} | {p['version']} | {lic} |")
        found = texts(p)
        found += standard(p.get("license"), found)
        if not found:
            missing.append(f"{p['name']} {p['version']} ({lic})")
        for name, text in found:
            groups.setdefault(text, []).append(f"{p['name']} {p['version']} ({name})")
    lines += ["", "## The texts", ""]
    for text, users in sorted(groups.items(), key=lambda g: g[1][0]):
        lines.append("### " + ", ".join(users))
        lines.append("")
        lines.append("```text")
        lines.append(text.replace("```", "'''").rstrip("\n"))
        lines.append("```")
        lines.append("")
    return "\n".join(lines), missing


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True)
    ap.add_argument("--check", metavar="FILE")
    ap.add_argument("--output", metavar="FILE", help="written as UTF-8 (else stdout)")
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    text, missing = render(a.target, linked(metadata(a.target, a.offline)))
    if missing:
        print("no licence text for: " + "; ".join(missing), file=sys.stderr)
        sys.exit(1)
    if a.check:
        if Path(a.check).read_text(encoding="utf-8") != text:
            print(f"{a.check} is not what Cargo.lock gives for {a.target}: make it again with\n"
                  f"  python3 scripts/third_party_licenses.py --target {a.target} --output {a.check}",
                  file=sys.stderr)
            sys.exit(1)
        print(f"{a.check}: up to date")
        return
    if a.output:
        Path(a.output).write_text(text, encoding="utf-8", newline="\n")
    else:
        sys.stdout.buffer.write(text.encode("utf-8"))


if __name__ == "__main__":
    main()
