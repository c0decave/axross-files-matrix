"""Check that a Linux binary needs no newer glibc than a floor (default
2.34: it then runs on AlmaLinux / RHEL 9, Ubuntu 22.04, Debian 12 and
later). Reads the ELF's version needs (.gnu.version_r) itself - no
binutils.

    python3 scripts/glibc_floor.py BINARY [FLOOR]

Exits 1 when the binary needs a newer glibc."""
import struct
import sys


def glibc_needed(path):
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        raise SystemExit(f"{path}: not a 64-bit little-endian ELF")
    shoff, = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum = struct.unpack_from("<HH", data, 0x3A)
    sections = []
    for i in range(shnum):
        o = shoff + i * shentsize
        _, kind, _, _, off, size, link = struct.unpack_from("<IIQQQQI", data, o)
        sections.append((kind, off, size, link))
    found = []
    for kind, off, size, link in sections:
        if kind != 0x6FFFFFFE:  # SHT_GNU_verneed
            continue
        strtab = sections[link][1]
        name = lambda at: data[strtab + at:data.index(b"\0", strtab + at)].decode()
        at = off
        while True:
            _, cnt, _, aux, nxt = struct.unpack_from("<HHIII", data, at)
            a = at + aux
            for _ in range(cnt):
                _, _, _, vname, anext = struct.unpack_from("<IHHII", data, a)
                v = name(vname)
                if v.startswith("GLIBC_") and v[6:7].isdigit():
                    found.append(tuple(int(x) for x in v[6:].split(".")))
                if not anext:
                    break
                a += anext
            if not nxt:
                break
            at += nxt
    return max(found) if found else None


def main():
    path = sys.argv[1]
    floor = tuple(int(x) for x in (sys.argv[2] if len(sys.argv) > 2 else "2.34").split("."))
    need = glibc_needed(path)
    shown = ".".join(map(str, need)) if need else "none"
    print(f"{path} needs glibc {shown} (floor {'.'.join(map(str, floor))})")
    if need and need > floor:
        print(f"::error::{path} needs glibc {shown}, newer than the floor", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
