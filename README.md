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
- **Links open with `Ctrl/Cmd`+click**: web and mail addresses in the
  browser or mail app, and wiki links in a tab. Hold `Ctrl/Cmd` and the
  pointer turns to a hand over one.
- **Lists that keep going**: `Enter` continues a list, a task list or a
  quote, and `Enter` on an empty item ends it. `Tab` and `Shift+Tab` indent
  and outdent. Task boxes tick with a click. A numbered list renumbers
  itself when an item is added, taken out, indented or outdented.
- **Brackets and quotes come in pairs**: typing `(`, `[`, `{`, `"`, `'` or
  `` ` `` adds its closer, typing the closer steps over it, and `Backspace`
  takes out an empty pair. With text selected, they (and `*`, `_`, `~`)
  wrap it. Source mode and code blocks type exactly what you press.
- **Paste a web address over selected text** to make it a link.
- **Tables you edit in place.** Type straight into the cells. In Live
  mode the cell you're in shows its Markdown. A `|` you type is escaped, so
  it stays text.
  - **Keys:**
    - `Tab` and `Shift+Tab` move between cells, and `Tab` past the last
      cell adds a row.
    - `Enter` goes down a column.
    - `Shift+Enter` starts a new line in the cell, saved as `<br>`.
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
  - `$…$` and `$$…$$` draw as formulas, `$…$` in table cells too. A lone
    `$5` stays a price. A line grows to fit a tall formula, such as a
    fraction.
  - Click a diagram or a display formula to edit its source with a live
    preview beneath it. An inline formula shows its source while the caret
    is in it: click it, or arrow into it.
  - A diagram that doesn't parse shows the error and its line.
- **Images** load from the document's folder.
  - **Paste an image** (a screenshot, or one copied in a browser), or drop
    image files on the text, and it goes in where you are: a pasted one is
    saved as `assets/pasted-….png` beside the document, and a dropped one
    from outside the document's folder is copied into `assets/`. Other
    dropped files open in tabs.
  - **Obsidian's embeds and sizes**: `![[photo.png]]` finds the picture by
    name as a wiki link finds its note, and `![[photo.png|300]]`,
    `![caption|300](photo.png)` or `|300x200` set its size, in the editor
    and in HTML and PDF exports.
  - **Web images** wait for a click, because fetching one tells its server
    the document was opened. **Settings** can allow them always.
  - **Network shares and other schemes** are never loaded.
- **GitHub-flavoured Markdown**: tables, task lists, strikethrough,
  footnotes, alerts (`> [!NOTE]` and the rest, as tinted cards), and fenced
  code with syntax highlighting in the theme's colours.
  - **A code block's header** has its language, which opens a menu to change
    it, and a copy button.
- **Obsidian's Markdown** too:
  - **Callouts** of every kind (`> [!info]`, `> [!bug] Its own title`, …),
    coloured by group as ZenNotes colours them.
  - **`#tags`** are pills.
  - **`[[Wiki links]]`** and `[[Note|labels]]`: `Ctrl/Cmd`+click opens the
    note, found by name in the document's folder or anywhere in the open
    folder. `[[#Heading]]` goes to a heading in the document. A link to no
    note is drawn faintly.
- **Completion as you type**: `[[` lists the notes in the open folder (or
  the document's), `![[` other files too, and `[[Note#` or `[[#` that
  note's headings. A `#` starting a word lists the tags this document and
  the open folder use. `Up`/`Down` choose, `Enter` or `Tab` takes one,
  `Esc` closes the list.
- **Embeds**: `![[Note]]` draws that note in place, on a card, and
  `![[Note#Heading]]` just that part of it. The card follows the note as it
  changes; its title shows the embed's source, and **Open** opens the note.
  `![[file.pdf]]` (or `#page=3`) draws a page of a PDF. HTML and PDF
  exports include embedded notes.
- **Previews**: hold `Ctrl/Cmd` over a wiki link for a moment to see its
  note.
- **Backlinks**: the sidebar's **Backlinks** tab lists the notes in the open
  folder that link to the document, each with the line its link is on; a
  click opens it there.
- **Frontmatter**: a YAML block opening the document (between `---` lines)
  is drawn as a card of its keys and values. HTML and PDF exports leave it
  out.
- **The look** follows ZenNotes' editor: code, formulas, callouts and the
  frontmatter on rounded cards, inline code in chips, and markup shown at
  the caret in a soft accent.
- **Folding**: a heading folds its section, a list item its nested items,
  and a callout everything under its title. Click the chevron that shows
  beside the line (or press `Ctrl/Cmd+Shift+[`); a folded block leaves a `…`
  that opens it again, as does `Ctrl/Cmd+Shift+]`, or the caret moving into
  it. **Fold All** and **Unfold All** are in the command palette.
- **Spell checking**, off until turned on in **Settings**: misspelt words
  get a wavy red line, and with grammar on, doubtful phrases a blue one.
  Right-click one for suggestions, to add the word to your dictionary
  (`<config>/mdedit/dictionary.txt`) or to ignore it in this document.
  Code, formulas, links' targets and tags are left alone. It's English
  only (US, UK, Canadian, Australian or Indian spelling), by
  [Harper](https://writewithharper.com), on this computer.
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
- **The sidebar** (`Ctrl/Cmd+Shift+L`) has three tabs:
  - **Outline** shows the document's headings, follows the caret, and jumps
    on a click.
  - **Files** shows the folder opened with `Ctrl/Cmd+Shift+K`. It reads each
    directory when you expand it.
  - **Backlinks** lists what links to the document (see above).
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
- **Typography**: Ogma, GitHub, Manuscript, Compact or Typewriter (ZenNotes'
  own: monospaced, with tables in a sans face), with the text size and a
  reading column or the full width.
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
| `Ctrl/Cmd+Shift+[` / `]` | Fold / unfold at the caret |
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
