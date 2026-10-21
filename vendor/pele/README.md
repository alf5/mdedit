# Pele

[Pele](https://pele.run) renders Mermaid diagrams to SVG. It is the one piece
of mdedit that is JavaScript rather than Rust: there is no Rust renderer that
keeps up with it. `src/mermaid.rs` imports this file through wasm-bindgen,
which copies it into the build's `snippets/` directory; nothing is fetched at
run time.

| | |
| --- | --- |
| Version | 1.8.0 |
| Source | npm `pele`, `dist/pele.js` (self-contained ES module, no imports) |
| Repository | https://github.com/obsidianmd/pele |
| npm integrity | `sha512-AYmHttE70cwNJVXdfPMCANGivFlmc3bGGpMG0GqH5S5vE3RgtEuLlkqbhRfe94Csv/ZEd1TUbv2dMUHpw4p1Bg==` |
| `pele.js` sha256 | `30d94f823025c873abb2ec3676e34124e543583731e228ca957f51ddda415a05` |
| License | MIT, in `LICENSE` |

## Updating

```sh
npm pack pele@<version>          # check the integrity against `npm view pele@<version> dist.integrity`
tar xzf pele-<version>.tgz
cp package/dist/pele.js package/LICENSE vendor/pele/
```

Then update the table above. Read the release notes for changes to `render`'s
options or to the `--pele-*` CSS variables, which `document.css` sets.
