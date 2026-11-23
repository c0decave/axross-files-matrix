"""Check a Windows program's PE subsystem: `python pe_subsystem.py EXE gui`.

2 is a GUI program (no console window opened for it), 3 a console one.
Exits 1 when the program is not of the kind asked for."""
import struct
import sys

KINDS = {"gui": 2, "console": 3}


def subsystem(path):
    with open(path, "rb") as f:
        data = f.read(4096)
    if data[:2] != b"MZ":
        raise SystemExit(f"{path}: not a PE program")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise SystemExit(f"{path}: no PE header")
    # COFF header (20 bytes) after the signature; the optional header's
    # Subsystem at offset 68 in both PE32 and PE32+
    return struct.unpack_from("<H", data, pe + 4 + 20 + 68)[0]


def main():
    path, want = sys.argv[1], sys.argv[2]
    got = subsystem(path)
    name = {v: k for k, v in KINDS.items()}.get(got, str(got))
    print(f"{path}: subsystem {got} ({name})")
    if got != KINDS[want]:
        print(f"expected a {want} program (subsystem {KINDS[want]})", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
