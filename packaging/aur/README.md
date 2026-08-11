# Arch package

Follows [Tauri's AUR guide](https://v2.tauri.app/distribute/aur/): the Arch
package is a repack of the Debian bundle rather than a second build. The only
departure is the source — the guide downloads a `.deb` from a GitHub release,
this `PKGBUILD` reads the one `cargo tauri build` just wrote into
`target/release/bundle/deb/`. Nothing is downloaded and no AUR remote is used.

## Build

```sh
cd ../..            # repository root
cargo tauri build   # produces target/release/bundle/deb/mdedit_<ver>_amd64.deb
cd packaging/aur
makepkg -f          # -i to install, -si to also pull in dependencies
```

The result is `mdedit-<pkgver>-<pkgrel>-<arch>.pkg.tar.zst`, installable with
`sudo pacman -U`. `makepkg` refuses to run as root; run it as your normal user.

## Files

| File           | Purpose                                                        |
| -------------- | -------------------------------------------------------------- |
| `PKGBUILD`     | Package recipe; extracts `data.tar.gz` from the local `.deb`.   |
| `mdedit.install` | Refreshes the icon cache and desktop database on install/remove. |
| `.SRCINFO`     | Generated metadata — `makepkg --printsrcinfo > .SRCINFO`.       |

## Releasing a new version

1. Bump `pkgver` in `PKGBUILD` to match `src-tauri/tauri.conf.json`, and reset
   `pkgrel=1` (bump `pkgrel` instead when only the packaging changed).
2. Regenerate `.SRCINFO`: `makepkg --printsrcinfo > .SRCINFO`.

The package ships the repository's `LICENSE` to
`/usr/share/licenses/mdedit/LICENSE`, since MIT is not among the licenses
provided by the `licenses` package.

Should this ever be published to the AUR, swap `package()`'s local extraction
for the guide's `source_x86_64`/`sha256sums_x86_64` pair pointing at the
released `.deb`.
