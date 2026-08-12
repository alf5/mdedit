# mdedit

A WYSIWYG Markdown editor built with **Tauri 2** (Rust backend) and **Leptos**
(Rust/WASM frontend). No JavaScript frameworks — the whole app is Rust.

![build](https://github.com/alf5/mdedit/actions/workflows/build.yml/badge.svg)

## Features

- **Multiple documents in tabs** — a single row of tabs with a `»` menu on the
  right when they no longer fit. New and Open always add a tab, so nothing is
  ever discarded; opening a file that's already open just focuses its tab.
  Each tab keeps its own undo history, view mode, caret and scroll position.
  Close with `Ctrl+W`, the tab's `✕`, or a middle click; a modified tab asks
  before it goes, and quitting walks the modified tabs one at a time. Closing
  the last tab leaves an empty window rather than quitting.
- **Recent files** — **File → Open Recent** keeps the last 10 documents,
  remembered between runs in `recent.json` under your config directory.
- **One instance** — opening a `.md` from the file manager (or a second
  `mdedit file.md` in a terminal) adds a tab to the running window instead of
  starting a second copy. Selecting several files at once opens them all.
- **WYSIWYG editing** — markdown is rendered with `pulldown-cmark` into an
  editable view; edits are serialized back to clean markdown source. Toggle to
  a raw **markdown source** mode any time (`Ctrl+Shift+M`).
- **GitHub-flavored markdown**: tables, task lists (clickable checkboxes),
  strikethrough, footnotes, alerts (`> [!NOTE]` …), fenced code blocks.
- **Mermaid diagrams**: a ```` ```mermaid ```` fence renders inline as SVG —
  flowcharts, sequence, state, class, ER, gantt, pie and the rest of the 23
  diagram types, drawn by the pure-Rust `mermaid-svg` crate (no `mermaid.js`,
  no Node). Click a diagram to edit its source, click away to re-render;
  colors follow the light/dark theme. A diagram that fails to parse shows the
  error above its source instead of disappearing. (Gantt charts draw no
  "today" marker: reading the clock means `SystemTime::now()`, which is a
  hard panic on `wasm32-unknown-unknown`.)
- **Formatting toolbar**: bold, italic, strikethrough, inline code, headings,
  bullet/numbered/task lists, blockquote, code block, links, images, tables,
  horizontal rules — works in both WYSIWYG and source mode.
- **Native menus**: File (New / Open / Open Recent / Save / Save As / Close
  Tab / Quit), Edit (undo, redo, clipboard, Find / Replace), View (source
  toggle, next/previous document), Insert (link, image, table, mermaid
  diagram, hr, table rows/columns), Format, Help.
- **Table editing**: insert row above/below and column left/right, delete
  row/column/table — from the toolbar (shown while the caret is in a table),
  the Insert menu, or a right-click context menu on any table cell.
- **Find & replace** (`Ctrl+F` / `Ctrl+H`) with match count, case toggle,
  replace one / all.
- **Undo/redo** with its own snapshot history (covers table edits, checkbox
  toggles and replace-all, which browser-native undo can't see) — toolbar
  buttons with live enabled state, menu items, `Ctrl+Z`/`Ctrl+Shift+Z`/`Ctrl+Y`.
- Window title shows the active tab (`notes.md - mdedit`, `*` when modified),
  word/char count status bar, light + dark theme, open files from the CLI:
  `mdedit notes.md draft.md`.
- **Drag & drop** markdown files onto the window to open them as tabs, and
  installed bundles register a **file association** for
  `.md`/`.markdown`/`.mdown`/`.mkd`, so "Open With → mdedit" and double-click
  work on all three platforms.
- **Smart paste**: pasting rich content (web pages, Word, Google Docs)
  converts the clipboard's HTML to markdown — `Ctrl+Shift+V` pastes plain
  text instead. **File → Import HTML…** converts an HTML file into a new
  markdown document. Scripts/styles are stripped, `&nbsp;` normalized, and
  embedded data-URI images degrade to their alt text.
- **Word import**: **File → Import Word Document…** converts a `.docx`
  (pure-Rust OOXML parser — headings, bold/italic/strikethrough,
  hyperlinks, nested bullet/numbered lists, tables, quotes, line breaks;
  images, footnotes and TOC field machinery are skipped).

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl/Cmd+N` | New file (in a new tab) |
| `Ctrl/Cmd+O` | Open file (in a new tab) |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+Shift+S` | Save As |
| `Ctrl/Cmd+W` | Close tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous document |
| `Ctrl/Cmd+PageDown` / `PageUp` | Next / previous document |
| `Ctrl/Cmd+Z` | Undo |
| `Ctrl/Cmd+Shift+Z` or `Ctrl+Y` | Redo |
| `Ctrl/Cmd+F` | Find |
| `Ctrl+H` (`Cmd+Alt+F` on macOS) | Replace |
| `Ctrl/Cmd+B` / `I` | Bold / Italic |
| `Ctrl/Cmd+Shift+X` | Strikethrough |
| `Ctrl/Cmd+E` | Inline code |
| `Ctrl/Cmd+K` | Insert link |
| `Ctrl/Cmd+1/2/3` / `Ctrl/Cmd+0` | Heading 1–3 / paragraph |
| `Ctrl/Cmd+V` / `Ctrl/Cmd+Shift+V` | Smart paste / plain paste |
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
