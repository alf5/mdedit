# mdedit

A WYSIWYG Markdown editor built with **Tauri 2** (Rust backend) and **Leptos**
(Rust/WASM frontend). No JavaScript frameworks — the whole app is Rust.

![build](https://github.com/alf5/mdedit/actions/workflows/build.yml/badge.svg)

## Features

- **WYSIWYG editing** — markdown is rendered with `pulldown-cmark` into an
  editable view; edits are serialized back to clean markdown source. Toggle to
  a raw **markdown source** mode any time (`Ctrl+Shift+M`).
- **GitHub-flavored markdown**: tables, task lists (clickable checkboxes),
  strikethrough, footnotes, alerts (`> [!NOTE]` …), fenced code blocks.
- **Formatting toolbar**: bold, italic, strikethrough, inline code, headings,
  bullet/numbered/task lists, blockquote, code block, links, images, tables,
  horizontal rules — works in both WYSIWYG and source mode.
- **Native menus**: File (New / Open / Save / Save As / Quit), Edit (undo,
  redo, clipboard, Find / Replace), View, Insert (link, image, table, hr,
  table rows/columns), Format, Help.
- **Table editing**: insert row above/below and column left/right, delete
  row/column/table — from the toolbar (shown while the caret is in a table),
  the Insert menu, or a right-click context menu on any table cell.
- **Find & replace** (`Ctrl+F` / `Ctrl+H`) with match count, case toggle,
  replace one / all.
- Window title shows the current file (`notes.md - mdedit`, `*` when
  modified), unsaved-changes prompt on close, word/char count status bar,
  light + dark theme, open a file from the CLI: `mdedit notes.md`.
- **Drag & drop** a markdown file onto the window to open it (with the usual
  unsaved-changes prompt), and installed bundles register a **file
  association** for `.md`/`.markdown`/`.mdown`/`.mkd`, so "Open With →
  mdedit" and double-click work on all three platforms.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl/Cmd+N` | New file |
| `Ctrl/Cmd+O` | Open file |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+Shift+S` | Save As |
| `Ctrl/Cmd+F` | Find |
| `Ctrl+H` (`Cmd+Alt+F` on macOS) | Replace |
| `Ctrl/Cmd+B` / `I` | Bold / Italic |
| `Ctrl/Cmd+Shift+M` | Toggle WYSIWYG / markdown source |
| `Tab` / `Shift+Tab` | Indent / outdent list item |

## Development

Prerequisites: Rust (stable), the `wasm32-unknown-unknown` target,
[trunk](https://trunkrs.dev) and [tauri-cli](https://tauri.app):

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk tauri-cli
```

Run in development mode:

```sh
cargo tauri dev
```

Build a stripped release binary and installers:

```sh
cargo tauri build
```

## CI

GitHub Actions builds stripped executables and installers on every push to
`main` for:

- **Linux** (x86_64): raw binary, `.deb`, `.rpm`, `.AppImage`
- **Windows** (x86_64): raw `.exe`, MSI and NSIS installers
- **macOS** (universal arm64+x86_64): raw binary, `.dmg`, `.app`

Tag a release (`git tag v0.1.0 && git push --tags`) to publish the artifacts
to a GitHub release.
