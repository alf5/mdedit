# Arch package

`PKGBUILD` builds mdedit from this repository's working tree with Cargo and
installs the binary, the desktop entry, the icons and the licence. Nothing is
downloaded apart from the crates Cargo fetches, and no AUR remote is used.

## Build

```sh
cd packaging/aur
makepkg -f          # -i to install, -si to also pull in dependencies
```

The result is `mdedit-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst`, installable with
`sudo pacman -U`. `makepkg` refuses to run as root; run it as your normal user.

## Files

| File             | Purpose                                                          |
| ---------------- | ---------------------------------------------------------------- |
| `PKGBUILD`       | Package recipe: `cargo build --release`, then install the files.  |
| `mdedit.install` | Refreshes the icon cache and desktop database on install/remove. |
| `.SRCINFO`       | Generated metadata: `makepkg --printsrcinfo > .SRCINFO`.          |

The desktop entry is `packaging/linux/mdedit.desktop`, shared with the RPM.

## Releasing a new version

1. Bump `pkgver` in `PKGBUILD` to match the workspace version in the root
   `Cargo.toml`, and reset `pkgrel=1` (bump `pkgrel` instead when only the
   packaging changed).
2. Regenerate `.SRCINFO`: `makepkg --printsrcinfo > .SRCINFO`.

Should this ever be published to the AUR, give `source` the release tarball
and build from `$srcdir` instead of the working tree.
