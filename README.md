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
  the last tab leaves an empty window rather than quitting. Right-click a tab
  to **Open Containing Folder** in the OS file manager (disabled until the
  document has a path).
- **Side by side** — split the window into two sides, each with its own tab
  strip, three ways: **View → Split Right** (`Ctrl/Cmd+\`), **Split Right**
  on a tab's right-click menu, or by dragging a tab onto the right (or left)
  half of the document. Once split, the same command reads **Move to Left /
  Right Side**; dragging a tab onto the other strip puts it there at the drop
  point, onto the other side's document puts it last, and along its own strip
  reorders it (`Esc` cancels a drag). **Join Sides** puts everything back on
  one side, and moving or closing a side's last tab does the same. Drag the
  divider to give either side 20–80% of the width; double-click it to make
  them equal again. The side you aren't typing in shows its document exactly
  as it was left and can be scrolled; click anywhere on it to edit there, with
  the caret where you clicked. The format and find bars span both sides and
  act on the side being edited.
- **Recent files** — **File → Open Recent** keeps the last 10 documents,
  remembered between runs in `recent.json` under your config directory.
- **One instance** — opening a `.md` from the file manager (or a second
  `mdedit file.md` in a terminal) adds a tab to the running window instead of
  starting a second copy. Selecting several files at once opens them all.
  A first launch from a terminal holds the shell until the window closes;
  `mdedit --detach` (with or without files) returns the prompt immediately.
  A second launch already does — it only forwards its files and exits.
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
  horizontal rules — works in both WYSIWYG and source mode. Shown only for
  markdown tabs, in the document column (not above the tab strip, and not
  for images, hex or plain text). In the rendered view, **Wide** toggles
  between a readable column and the full pane, and a style menu switches
  Ogma / GitHub / Manuscript / Compact / Typewriter typography.
  **View → Hierarchical Numbering** overlays outline-style heading and list
  numbers on any of those. The scrollbar stays on the window's right edge.
- **Native menus**: File (New / Open / Open Folder / Open Recent / Save /
  Save As / Export as HTML / Export as PDF / Print / Close Tab / Quit),
  Edit (undo, redo, clipboard, Find / Replace), View (project pane, markdown
  source, full-width, hierarchical numbering, markdown style, hex overwrite —
  each ticked when on — split right / move to the other side, join sides,
  next/previous document), Insert (link, image, table,
  mermaid diagram, hr, table rows/columns), Format, Help.
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
  `mdedit notes.md draft.md`, or `mdedit --detach notes.md` to background a
  first launch.
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
- **Project pane** (optional): **File → Open Folder…** (`Ctrl/Cmd+Shift+K`)
  opens a directory tree down the left-hand side; `Ctrl/Cmd+Shift+P` shows and
  hides it. Directories are read when you expand them, so a folder with a
  large `node_modules` in it costs nothing until you look inside. The folder is
  not remembered between launches, in keeping with tabs.

  Anything in the tree opens in a tab, and what a file *is* decides what it
  gets: markdown gets the editor; **images** (including SVG) are displayed,
  read-only; anything else is sniffed, and **binaries get a hex editor** while
  the rest is **editable plain text**. Text and binaries stop at 1 MB and
  images at 16 MB — past that the tab says so rather than pretending the file
  is empty. Formatting, export and the source toggle are disabled for anything
  that isn't markdown, in the toolbar and in the menus, so their accelerators
  are inert too.
- **Hex editor**: arrow keys, `PageUp`/`PageDown`, `Home`/`End`, and hex digits
  type over the byte under the cursor — high nibble then low, which is one
  `Ctrl+Z` to take back. `Insert` (or `Ctrl/Cmd+Shift+O`, or the `OVR`/`INS`
  button in the status bar) switches between **overwrite** and **insert**;
  overwrite is the default and cannot change the file's length, so it cannot
  shift every offset in a structured binary. In insert mode `Backspace` and
  `Delete` remove bytes. Undo is operation-based, and saving writes the bytes
  back where they came from — there is no Save As for a binary.
- **External change detection**: if something else writes to an open file — a
  `git pull`, another editor, a script — the tab says so and asks what to do:
  **Reload** (take the file's version), **Merge** (three-way merge against
  what the tab and the file last agreed on, with `<<<<<<<` conflict markers
  and a drop into source mode when the two really clash), or **Keep Mine**.
  Only the tab you are on is asked; a background tab carries a `≠` marker
  until you switch to it, and `Esc` postpones the question. A plain **Save**
  will not overwrite a file that changed underneath it — it asks the same
  question instead. Rewrites that leave the content identical (`touch`, a
  no-op save, a `git checkout` of the same bytes) are ignored.
- **HTML export**: **File → Export as HTML…** (`Ctrl/Cmd+Shift+E`) writes one
  self-contained `.html` file — the stylesheet is embedded and every mermaid
  diagram is already inline SVG, so it opens offline in any browser with no
  scripts, fonts or network fetches, and follows the reader's light/dark
  preference. Diagrams keep their source in a `data-mermaid` attribute, so an
  exported file re-imported through **Import HTML** comes back as
  ```` ```mermaid ```` fences rather than unrecoverable SVG. Relative image
  links are left as written, so they resolve when the `.html` is saved beside
  the markdown it came from.
- **PDF export and printing**: **File → Export as PDF…** writes a PDF, and
  **File → Print…** (`Ctrl/Cmd+P`) opens the system print dialog — from which
  "print to file" is a second route to a PDF, one that lets you pick the paper.
  Both paginate the *same* document the HTML export writes, so a PDF is the
  exported page with page breaks in it and the three renderings cannot drift.
  There is no second renderer and no bundled browser: the export is printed by
  whatever machinery the platform's webview already owns — `WebKitPrintOperation`
  on Linux, `PrintToPdf` on Windows, `NSPrintOperation` on macOS — in a hidden
  window of its own, so no editor chrome reaches the page.

  The page rules live in `document.css` under `@media print`, which means an
  exported `.html` printed from a browser breaks its pages the same way: the
  light palette is pinned (a document composed in dark mode would otherwise
  print white ink on a background the printer drops), backgrounds are kept so
  code blocks, table headers and alerts stay distinguishable, headings are not
  left stranded at the foot of a page, tables, quotes, code blocks and diagrams
  avoid being split, over-long code lines wrap rather than being clipped, and
  a diagram taller than the text block is scaled down to it.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Ctrl/Cmd+N` | New file (in a new tab) |
| `Ctrl/Cmd+O` | Open file (in a new tab) |
| `Ctrl/Cmd+S` | Save |
| `Ctrl/Cmd+Shift+S` | Save As |
| `Ctrl/Cmd+Shift+E` | Export as HTML |
| `Ctrl/Cmd+P` | Print (or print to file, for a PDF) |
| `Ctrl/Cmd+Shift+K` | Open folder (project pane) |
| `Ctrl/Cmd+Shift+P` | Show / hide the project pane |
| `Insert` or `Ctrl/Cmd+Shift+O` | Hex editor: overwrite / insert mode |
| `Ctrl/Cmd+W` | Close tab |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous document |
| `Ctrl/Cmd+PageDown` / `PageUp` | Next / previous document |
| `Ctrl/Cmd+\` | Split right / move the tab to the other side |
| `Ctrl/Cmd+Z` | Undo |
| `Ctrl/Cmd+Shift+Z` or `Ctrl+Y` | Redo |
| `Ctrl/Cmd+F` | Find |
| `Ctrl+H` (`Cmd+Alt+F` on macOS) | Replace |
| `Ctrl/Cmd+B` / `I` | Bold / Italic |
| `Ctrl/Cmd+Shift+X` | Strikethrough |
| `Ctrl/Cmd+E` | Inline code |
| `Ctrl/Cmd+K` | Insert link |
| `Ctrl/Cmd+1`–`6` / `Ctrl/Cmd+0` | Heading 1–6 / paragraph |
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
