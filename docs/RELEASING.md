# Releasing axross-files-matrix

Back to the [README](../README.md).

A release is built by CI and signed by the maintainer: the signing key never
goes to CI, into a repository or into a package. The program's update
(src/update.rs) takes no release without a valid signature of its
`SHA256SUMS` by the key pinned in the program (`UPDATE_PUBLIC_KEY`, in
`TRUSTED_KEYS`).

## The key

The release key is pinned: `UPDATE_PUBLIC_KEY` in `src/update.rs` holds
its Ed25519 public half (base64
`7Q1+QXpJXtHCjQa9yR65Ht9Bk/AURDLu7vfeJYN1kT4=`), and `TRUSTED_KEYS` lists
it. Its private half stays on the maintainer's machine, outside every
checkout, in a folder of mode 0700; CI never sees it. A test checks the
pinned key; another that an all-zero key verifies nothing (the program
then checks for no update and only points to the releases page).

- A new key (only to replace or rotate this one) is made on the
  maintainer's machine: `scripts/sign_release.py keygen --key
  PATH/afm-update-signing.pem`. It refuses a path inside a git work tree
  and a folder others may read; it writes the private key (PEM, PKCS#8,
  mode 0600) and prints the public half as base64 and as Rust bytes
  (`scripts/sign_release.py pubkey --key KEY` prints them again). Back the
  private key up offline: without it no release can be signed for the
  copies installed.
- Rotating: pin the new key beside the old one in `TRUSTED_KEYS` and
  release with the old one; from the release after that, sign with the new
  key; drop the old one once the releases still pinning only it are no
  longer in use.

## Steps

1. The version in `Cargo.toml`, a `## X.Y.Z - DATE` section in
   `CHANGELOG.md`; the tests (`cargo test`) and the supply-chain gate
   (`security/supply-chain-cargo.toml`: every crate of `Cargo.lock`
   reviewed in `security/cargo_baseline/`) pass; `THIRD_PARTY_LICENSES.md`
   made again (`scripts/third_party_licenses.py --target
   x86_64-unknown-linux-gnu --output THIRD_PARTY_LICENSES.md`).
2. Tag `vX.Y.Z` and push the tag: `.github/workflows/release.yml` runs the
   tests and the gate again, builds Linux (a binary and an AppImage), macOS
   (a universal binary and a DMG) and Windows, writes `SHA256SUMS` and the
   provenance attestation, and makes a **draft** release.
3. On the maintainer's machine (Python 3 with `cryptography`, the GitHub
   CLI logged in):

   ```sh
   scripts/sign_release.py release vX.Y.Z --key PATH/afm-update-signing.pem
   ```

   It downloads the draft's `SHA256SUMS`, checks every file of the draft
   against it, refuses a key that is not pinned, signs, checks the
   signature and uploads `SHA256SUMS.sig`.
4. Look at the draft (the files, the notes), then publish it:
   `scripts/sign_release.py release vX.Y.Z --key ... --publish` does steps 3
   and 4 in one, or `gh release edit vX.Y.Z --draft=false --latest`.
5. crates.io: `cargo publish` (the screenshots and snapshots are left out of
   the package, see `exclude` in `Cargo.toml`).

A release published without `SHA256SUMS.sig` is never offered by the
update; signing later (step 3 on the published release) is enough.
