# mdedit

A Markdown editor that shows your writing, not your markup.

![build](https://github.com/alf5/mdedit/actions/workflows/build.yml/badge.svg)

mdedit 2 is a native app written in Rust on [GPUI](https://gpui-kit.com),
with no webview and no JavaScript. The Markdown file is the document: what
you type is what gets saved, byte for byte. The editor draws that text
formatted, so headings look like headings and `**bold**` looks bold, but
nothing is converted back and forth behind your back.

## Writing

- **Three ways to see a document**, switched in the status bar or with
  `Ctrl/Cmd+Shift+M`:
  - **Rendered** hides the Markdown.
  - **Live** shows it only on the lines you're editing.
  - **Source** shows it all, in a monospace face.

  Each tab keeps its own mode. **Settings** chooses the one new documents
  start in.
- **No toolbar in the way.**
  - **Selecting text** brings up a small bar with bold, italic, strike, code,
    link, heading, quote and list buttons.
  - **Typing `/`** at the start of an empty line opens a menu of blocks:
    headings, lists, tasks, quote, code, table, diagram, math and divider.
    Type to filter it; `h2` finds Heading 2.
  - **The command palette** (`Ctrl/Cmd+Shift+P`) has every command and its
    shortcut.
- **Lists that keep going**: `Enter` continues a list, a task list or a
  quote, and `Enter` on an empty item ends it. `Tab` and `Shift+Tab` indent
  and outdent. Task boxes tick with a click.
- **Tables you edit in place.** Type straight into the cells. In Live
  mode the cell you're in shows its Markdown. A `|` you type is escaped, so
  it stays text.
  - **Keys:**
    - `Tab` and `Shift+Tab` move between cells, and `Tab` past the last
      cell adds a row.
    - `Enter` goes down a column.
    - `Up` and `Down` change row.
    - `Left` and `Right` stop at the cell's edges.
    - `Esc` leaves the table.
  - **Hover a table** for its controls:
    - `+` rails on the right and bottom edges add a column or a row.
    - A grip above each header, or left of each row, drags it to a new
      place.
    - A header's right border drags to resize the column. Widths are saved
      in a `<!-- zen:cols=… -->` comment after the table, the format
      [ZenNotes-rs](https://github.com/alf5/ZenNotes-rs) uses.
  - **Right-click a cell** to:
    - insert, move or duplicate rows and columns
    - align a column (the current one is ticked)
    - sort by a column
    - copy the table as Markdown
    - duplicate or delete the table
    - delete a row or column
  - **Columns line up in the source:** the table is padded when you leave
    it.
- **Diagrams and formulas.**
  - A ```` ```mermaid ```` fence draws as a diagram, using
    [bezel-mermaid](https://crates.io/crates/bezel-mermaid), a pure-Rust port
    of [Pele](https://pele.run).
  - `$…$` and `$$…$$` draw as formulas. A lone `$5` stays a price.
  - Click either one to edit its source with a live preview beneath it.
  - A diagram that doesn't parse shows the error and its line.
- **Images** load from the document's folder.
  - **Web images** wait for a click, because fetching one tells its server
    the document was opened. **Settings** can allow them always.
  - **Network shares and other schemes** are never loaded.
- **GitHub-flavoured Markdown**: tables, task lists, strikethrough,
  footnotes, alerts (`> [!NOTE]` and the rest, each with its own colour), and
  fenced code with syntax highlighting.
- **Find and replace** (`Ctrl/Cmd+F`, `Ctrl+H` or `Cmd+Alt+F`), with a match
  count, case matching, and replace one or all.
- **Smart paste.**
  - **Rich content** from web pages, Word or Google Docs arrives as Markdown.
    `Ctrl/Cmd+Shift+V` pastes plain text instead.
  - **Diagrams and formulas** copied out of an mdedit HTML export come back
    as their source.
- **Undo** takes back a burst of typing at a time, and keeps 200 steps per
  tab.

## Documents

- **Tabs** live in the title bar.
  - **New** and **Open** always add a tab. Opening a file that's already
    open just goes to its tab.
  - **A modified tab** asks before it closes, and quitting walks you through
    each modified tab.
  - **Right-click a tab** for Close Others, Split Right, or Open Containing
    Folder.
- **Side by side**: `Ctrl/Cmd+\` (or dragging a tab) splits the window. Each
  side has its own tabs, and every side can be edited.
- **The sidebar** (`Ctrl/Cmd+Shift+L`) has two tabs:
  - **Outline** shows the document's headings, follows the caret, and jumps
    on a click.
  - **Files** shows the folder opened with `Ctrl/Cmd+Shift+K`. It reads each
    directory when you expand it.
- **Quick Open** (`Ctrl/Cmd+P`) searches recent files and the open folder.
- **Anything opens.**
  - **Markdown** gets the editor.
  - **Other text** gets a code editor with syntax highlighting.
  - **Images** are shown.
  - **Binaries** get a hex editor, in overwrite mode by default. `Insert`
    switches it to insert mode.
  - **Size limits:** text and binaries stop at 1 MB, and images at 16 MB.
- **Changes on disk.** When something else writes to an open file (a
  `git pull`, another editor), a banner offers three choices:
  - **Reload** takes the file's version.
  - **Merge** does a three-way merge with conflict markers.
  - **Keep Mine** keeps the tab's version.

  **Save** won't overwrite a file that changed underneath it.
- **One window.** Opening a file from the file manager, or running
  `mdedit file.md` again, adds a tab to the running window. `mdedit --detach`
  returns the shell prompt at once.
- **File associations** for `.md`, `.markdown`, `.mdown` and `.mkd` come with
  the installers.

## Export

- **HTML** (`Ctrl/Cmd+Shift+E`): a single self-contained file.
  - The stylesheet is embedded, and diagrams and formulas are already drawn,
    so it opens offline with no scripts.
  - It follows the reader's light or dark setting.
  - **Import HTML** turns it back into Markdown, diagrams and formulas
    included.
- **PDF**, typeset in-process by [Typst](https://typst.app).
  - Headings stay with what follows them, and table headers repeat.
  - Diagrams and formulas are vector graphics.
- **Print** writes that PDF to a private temporary file and opens it in the
  system's viewer to print.
- **Import**: HTML files and Word documents (`.docx`) become new Markdown
  documents.

## Looks

**Settings** (`Ctrl/Cmd+,` or the gear in the title bar) holds everything,
applies it at once, and remembers it:

- **Themes**: 34 colour themes, shown as swatches, or **Match System**.
  - **The families:** Apple, Gruvbox, Catppuccin, GitHub, Solarized, One,
    Nord, Tokyo Night, Kanagawa, Black Metal and Rosé Pine.
  - **Where they apply:** the theme colours the window, the document and HTML
    exports. PDFs stay on white.
- **Typography**: Ogma, GitHub, Manuscript, Compact or Typewriter, with the
  text size and a reading column or the full width.
- **Numbered headings**: headings can be numbered like an outline (1., 1.1.,
  …).

Inter, Source Serif 4 and JetBrains Mono are bundled, so mdedit looks the
same on every system.

## Keyboard shortcuts

`Ctrl` on Linux and Windows, `Cmd` on macOS.

| Shortcut | Action |
| --- | --- |
| `Ctrl/Cmd+Shift+P` | Command palette |
| `Ctrl/Cmd+P` | Quick Open |
| `Ctrl/Cmd+N` / `O` | New / Open |
| `Ctrl/Cmd+S` / `Shift+S` | Save / Save As |
| `Ctrl/Cmd+W` | Close tab |
| `Ctrl/Cmd+Q` | Quit |
| `Ctrl/Cmd+,` | Settings |
| `Ctrl/Cmd+Shift+K` | Open folder |
| `Ctrl/Cmd+Shift+L` | Show / hide the sidebar |
| `Ctrl/Cmd+Shift+E` | Export as HTML |
| `Ctrl/Cmd+\` | Split right / move the tab to the other side |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous tab |
| `Ctrl/Cmd+PageDown` / `PageUp` | Next / previous tab |
| `Ctrl/Cmd+Shift+M` | Toggle Markdown source |
| `Ctrl/Cmd+B` / `I` | Bold / italic |
| `Ctrl/Cmd+Shift+X` | Strikethrough |
| `Ctrl/Cmd+E` | Inline code |
| `Ctrl/Cmd+K` | Link |
| `Ctrl/Cmd+1`–`6` / `0` | Heading 1–6 / paragraph |
| `Ctrl/Cmd+Z` | Undo |
| `Ctrl/Cmd+Shift+Z` or `Ctrl+Y` | Redo |
| `Ctrl/Cmd+V` / `Shift+V` | Smart paste / plain paste |
| `Ctrl/Cmd+F` | Find |
| `Ctrl+H` (`Cmd+Alt+F` on macOS) | Replace |
| `F3` / `Shift+F3` | Next / previous match |
| `Tab` / `Shift+Tab` | Indent / outdent; next / previous table cell |
| `Insert` or `Ctrl/Cmd+Shift+O` | Hex editor: overwrite / insert |

## Install

[Releases](https://github.com/alf5/mdedit/releases) have:

| Platform | Downloads |
| --- | --- |
| Linux | `.deb`, `.rpm`, `.AppImage`, a raw binary |
| Windows | NSIS and MSI installers, a raw `.exe` |
| macOS | `.dmg`, `.app`, a universal binary |

On Arch, `packaging/aur` builds a package from a checkout.

## Development

You need Rust 1.92 or later. On Linux you also need the development packages
for xcb, xkbcommon (with its X11 part), Wayland, fontconfig and Vulkan. On
Debian and Ubuntu that is:

```sh
sudo apt install libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libfontconfig-dev libvulkan-dev
```

Building and testing:

```sh
cargo run -p mdedit                         # the app
cargo test --workspace                      # all tests; the UI tests need no display
cargo run -p mdedit --no-default-features   # without PDF/Print (builds much faster)
```

The repository has three crates:

- `crates/mdedit-core`: the document model and everything that isn't UI,
  such as Markdown structure, editing commands, tables, themes, merge, import
  and export. It is unit-tested.
- `crates/mdedit-pdf`: Markdown to Typst to PDF.
- `crates/mdedit`: the GPUI app.

Engineering notes are in `docs/v2-notes.md`.

### Packaging

Installers come from
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager), and the
RPM from [cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm):

```sh
cargo build --release -p mdedit
cargo packager --release -p mdedit             # into target/packages
cargo generate-rpm -p crates/mdedit            # Linux only
```

CI builds the installers on every push for Linux, Windows and macOS.
Tagging a release (`git tag v2.0.0 && git push --tags`) attaches them to a
GitHub release.

## Licence

MIT. The bundled fonts are under the SIL Open Font Licence.
