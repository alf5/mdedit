//! PDF export: markdown typeset with Typst, in process, in pure Rust.
//!
//! The markdown is written out as Typst markup (`to_typst`) and compiled
//! against a small in-memory world: the app's bundled fonts, and the
//! pictures the document needs — images from its folder, mermaid diagrams
//! and formulas drawn as SVG (the same drawings the editor shows).
//!
//! Text always goes into the markup as Typst *strings* (`#"…";`), never as
//! markup, so nothing a document contains can be read as Typst code.
//!
//! The page rules follow `document.css`'s `@media print`: a light palette,
//! 18 × 16 mm margins, headings kept with what follows them, table headers
//! repeated on each page.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use mdedit_core::mermaid::{self, DrawOptions};
use mdedit_core::prefs::Style;
use mdedit_core::{math, os};
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd,
};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

/// How the PDF should look, and what it can draw with.
pub struct Options {
    pub style: Style,
    /// Outline numbers on headings.
    pub numbered: bool,
    /// US Letter rather than A4.
    pub letter: bool,
    /// Font files the document may use (Inter, Source Serif 4, JetBrains
    /// Mono).
    pub fonts: Vec<&'static [u8]>,
    /// The document's folder, for relative images.
    pub base_dir: Option<PathBuf>,
    /// How diagrams are drawn; `None` leaves them as code.
    pub diagrams: Option<DrawOptions>,
}

/// Typeset `md` as a PDF.
pub fn markdown_to_pdf(md: &str, title: &str, opts: &Options) -> Result<Vec<u8>, String> {
    let mut files = HashMap::new();
    let body = to_typst(md, opts, &mut files);
    let source = preamble(title, opts) + &body;
    let world = DocWorld::new(source, files, &opts.fonts);
    let doc: PagedDocument = typst::compile(&world).output.map_err(|errs| {
        errs.iter()
            .map(|e| e.message.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    typst_pdf::pdf(&doc, &typst_pdf::PdfOptions::default()).map_err(|errs| {
        errs.iter()
            .map(|e| e.message.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// A Typst string literal: text that can't be read as markup.
fn lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 6);
    out.push_str("#\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push_str("\";");
    out
}

/// A Typst string expression (no leading `#`).
fn string(s: &str) -> String {
    lit(s)
        .trim_start_matches('#')
        .trim_end_matches(';')
        .to_string()
}

fn preamble(title: &str, opts: &Options) -> String {
    let (font, size, leading, fill) = match opts.style {
        Style::GitHub => ("Inter", "10.5pt", "0.75em", "#1f2328"),
        Style::Ogma => ("Inter", "10.5pt", "0.8em", "#3b4148"),
        Style::Manuscript => ("Source Serif 4", "11.5pt", "0.85em", "#1f2328"),
        Style::Compact => ("Inter", "9.5pt", "0.55em", "#1f2328"),
        Style::Typewriter => ("JetBrains Mono", "9.5pt", "0.75em", "#1f2328"),
    };
    let paper = if opts.letter { "us-letter" } else { "a4" };
    let numbering = if opts.numbered {
        "#set heading(numbering: \"1.1.\")\n"
    } else {
        ""
    };
    format!(
        r##"#set document(title: {title})
#set page(paper: "{paper}", margin: (x: 16mm, y: 18mm), numbering: "1")
#set text(font: ("{font}", "Inter"), size: {size}, fill: rgb("{fill}"))
#set par(leading: {leading}, spacing: 1.1em)
{numbering}#show raw: set text(font: "JetBrains Mono", size: 0.88em)
#show raw.where(block: true): it => block(fill: rgb("#f6f8fa"), stroke: 0.5pt + rgb("#d1d9e0"), inset: 8pt, radius: 4pt, width: 100%, breakable: false, it)
#show raw.where(block: false): it => box(fill: rgb("#eff1f3"), inset: (x: 2.5pt), outset: (y: 2pt), radius: 2pt, it)
#show heading: set block(sticky: true, above: 1.3em, below: 0.65em)
#show heading.where(level: 1): set text(size: 1.85em)
#show heading.where(level: 2): set text(size: 1.45em)
#show heading.where(level: 3): set text(size: 1.2em)
#show link: set text(fill: rgb("#0969da"))
#set table(stroke: 0.5pt + rgb("#d1d9e0"), inset: 6pt)
#show table.cell.where(y: 0): set text(weight: "bold")
#let bq(body) = block(stroke: (left: 2.5pt + rgb("#d1d9e0")), inset: (left: 10pt, y: 3pt), breakable: false, text(fill: rgb("#59636e"), body))
#let alert(color, title, body) = block(stroke: (left: 2.5pt + rgb(color)), inset: (left: 10pt, y: 3pt), breakable: false)[#text(fill: rgb(color), weight: "bold", title)#linebreak()#body]
#let task(done) = box(width: 0.75em, height: 0.75em, baseline: 0.1em, radius: 1.5pt, stroke: if done {{ none }} else {{ 0.6pt + rgb("#59636e") }}, fill: if done {{ rgb("#0969da") }} else {{ none }})
"##,
        title = string(title)
    )
}

/// Writes markdown events out as Typst.
struct Writer<'a> {
    out: String,
    opts: &'a Options,
    files: &'a mut HashMap<String, Vec<u8>>,
    footnotes: HashMap<String, String>,
    /// Text gathered for a code block or an image's alt text.
    capture: Option<String>,
    code: Option<String>,
    image: Option<String>,
    table_cols: usize,
}

impl Writer<'_> {
    fn file(&mut self, ext: &str, bytes: Vec<u8>) -> String {
        let path = format!("/asset/{}.{ext}", self.files.len());
        self.files.insert(path.clone(), bytes);
        path
    }

    fn math(&mut self, src: &str, display: bool) {
        match math::draw(src, display, 10.5, [0x1f, 0x23, 0x28]) {
            Ok(m) => {
                let path = self.file("svg", m.svg.into_bytes());
                let (h, base) = (m.height_pt, m.height_pt - m.baseline_pt);
                if display {
                    self.out.push_str(&format!(
                        "\n\n#align(center, image({}, height: {h:.2}pt))\n\n",
                        string(&path)
                    ));
                } else {
                    self.out.push_str(&format!(
                        "#box(baseline: {base:.2}pt, image({}, height: {h:.2}pt));",
                        string(&path)
                    ));
                }
            }
            Err(_) => self.out.push_str(&format!("#raw({});", string(src))),
        }
    }

    fn code_block(&mut self, lang: &str, code: &str) {
        if lang.eq_ignore_ascii_case("mermaid")
            && let Some(d) = &self.opts.diagrams
            && let Ok(drawing) = mermaid::draw(code, d)
        {
            let path = self.file("svg", drawing.svg.into_bytes());
            let w = drawing.width * 0.75;
            let width = if w > 480. {
                "100%".to_string()
            } else {
                format!("{w:.1}pt")
            };
            self.out.push_str(&format!(
                "\n\n#align(center, image({}, width: {width}))\n\n",
                string(&path)
            ));
            return;
        }
        let lang = if lang.is_empty() {
            "none".to_string()
        } else {
            string(lang)
        };
        self.out.push_str(&format!(
            "\n\n#raw(block: true, lang: {lang}, {})\n\n",
            string(code.trim_end_matches('\n'))
        ));
    }

    fn image(&mut self, url: &str, alt: &str) {
        let os::ImageRef::Local(path) = os::resolve_image(url, self.opts.base_dir.as_deref())
        else {
            self.out.push_str(&lit(alt));
            return;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match (std::fs::read(&path), ext.as_str()) {
            (Ok(bytes), "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp") => {
                let vpath = self.file(&ext, bytes);
                self.out
                    .push_str(&format!("#box(image({}, width: auto));", string(&vpath)));
            }
            _ => self.out.push_str(&lit(alt)),
        }
    }

    fn event(&mut self, event: Event) {
        if let Some(buf) = &mut self.capture {
            match &event {
                Event::Text(t) | Event::Code(t) => {
                    buf.push_str(t);
                    return;
                }
                Event::End(TagEnd::CodeBlock) | Event::End(TagEnd::Image) => {}
                Event::SoftBreak | Event::HardBreak => {
                    buf.push(' ');
                    return;
                }
                _ => return,
            }
        }
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) => self.out.push_str(&lit(&t)),
            Event::Code(t) => self.out.push_str(&format!("#raw({});", string(&t))),
            Event::InlineMath(t) => self.math(&t, false),
            Event::DisplayMath(t) => self.math(&t, true),
            Event::SoftBreak => self.out.push_str(&lit(" ")),
            Event::HardBreak => self.out.push_str("#linebreak();"),
            Event::Rule => self
                .out
                .push_str("\n\n#line(length: 100%, stroke: 0.5pt + rgb(\"#d1d9e0\"))\n\n"),
            Event::TaskListMarker(done) => self.out.push_str(&format!("#task({done})#h(0.35em);")),
            Event::FootnoteReference(label) => {
                let body = self
                    .footnotes
                    .get(label.as_ref())
                    .cloned()
                    .unwrap_or_default();
                self.out.push_str(&format!("#footnote[{}];", body.trim()));
            }
            Event::InlineHtml(h) | Event::Html(h) => {
                let tag = h.trim().to_ascii_lowercase();
                if tag.starts_with("<br") {
                    self.out.push_str("#linebreak();");
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                let n = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.out.push_str(&format!("\n\n#heading(level: {n})["));
            }
            Tag::BlockQuote(kind) => {
                let alert = kind.map(|k| match k {
                    BlockQuoteKind::Note => ("#0969da", "Note"),
                    BlockQuoteKind::Tip => ("#1a7f37", "Tip"),
                    BlockQuoteKind::Important => ("#8250df", "Important"),
                    BlockQuoteKind::Warning => ("#9a6700", "Warning"),
                    BlockQuoteKind::Caution => ("#d1242f", "Caution"),
                });
                match alert {
                    Some((color, title)) => self.out.push_str(&format!(
                        "\n\n#alert({}, {})[",
                        string(color),
                        string(title)
                    )),
                    None => self.out.push_str("\n\n#bq["),
                }
            }
            Tag::CodeBlock(kind) => {
                self.code = Some(match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                });
                self.capture = Some(String::new());
            }
            Tag::List(Some(start)) => self.out.push_str(&format!("\n\n#enum(start: {start}, ")),
            Tag::List(None) => self.out.push_str("\n\n#list("),
            Tag::Item => self.out.push('['),
            Tag::Table(aligns) => {
                self.table_cols = aligns.len().max(1);
                let align: Vec<&str> = aligns
                    .iter()
                    .map(|a| match a {
                        Alignment::Left | Alignment::None => "left",
                        Alignment::Center => "center",
                        Alignment::Right => "right",
                    })
                    .collect();
                self.out.push_str(&format!(
                    "\n\n#table(columns: {}, align: ({},), ",
                    self.table_cols,
                    align.join(", ")
                ));
            }
            Tag::TableHead => self.out.push_str("table.header(repeat: true, "),
            Tag::TableRow => {}
            Tag::TableCell => self.out.push('['),
            Tag::Emphasis => self.out.push_str("#emph["),
            Tag::Strong => self.out.push_str("#strong["),
            Tag::Strikethrough => self.out.push_str("#strike["),
            Tag::Link { dest_url, .. } => {
                if os::is_openable_link(&dest_url) {
                    self.out.push_str(&format!("#link({})[", string(&dest_url)));
                } else {
                    self.out.push_str("#[");
                }
            }
            Tag::Image { dest_url, .. } => {
                self.image = Some(dest_url.to_string());
                self.capture = Some(String::new());
            }
            Tag::FootnoteDefinition(_) => {
                // Written where they're referenced; see `to_typst`.
                self.capture = Some(String::new());
                self.code = None;
                self.image = None;
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.out.push_str("\n\n"),
            TagEnd::Heading(_) => self.out.push_str("]\n\n"),
            TagEnd::BlockQuote(_) => self.out.push_str("]\n\n"),
            TagEnd::CodeBlock => {
                let code = self.capture.take().unwrap_or_default();
                let lang = self.code.take().unwrap_or_default();
                self.code_block(&lang, &code);
            }
            TagEnd::List(_) => self.out.push_str(")\n\n"),
            TagEnd::Item => self.out.push_str("], "),
            TagEnd::Table => self.out.push_str(")\n\n"),
            TagEnd::TableHead => self.out.push_str("), "),
            TagEnd::TableRow => {}
            TagEnd::TableCell => self.out.push_str("], "),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.out.push(']')
            }
            TagEnd::Image => {
                let alt = self.capture.take().unwrap_or_default();
                if let Some(url) = self.image.take() {
                    self.image(&url, &alt);
                }
            }
            TagEnd::FootnoteDefinition => {
                self.capture = None;
            }
            _ => {}
        }
    }
}

/// The document's body as Typst markup; pictures it needs go in `files`.
pub fn to_typst(md: &str, opts: &Options, files: &mut HashMap<String, Vec<u8>>) -> String {
    let parser_opts = mdedit_core::syntax::options();
    // Footnote definitions first, so a reference can carry its text.
    let mut footnotes = HashMap::new();
    {
        let mut current: Option<(String, Vec<Event>)> = None;
        for ev in Parser::new_ext(md, parser_opts) {
            match ev {
                Event::Start(Tag::FootnoteDefinition(label)) => {
                    current = Some((label.to_string(), Vec::new()))
                }
                Event::End(TagEnd::FootnoteDefinition) => {
                    if let Some((label, events)) = current.take() {
                        let mut w = Writer {
                            out: String::new(),
                            opts,
                            files,
                            footnotes: HashMap::new(),
                            capture: None,
                            code: None,
                            image: None,
                            table_cols: 0,
                        };
                        for e in events {
                            w.event(e);
                        }
                        footnotes.insert(label, w.out);
                    }
                }
                e => {
                    if let Some((_, events)) = &mut current {
                        events.push(e.into_static());
                    }
                }
            }
        }
    }
    let mut w = Writer {
        out: String::new(),
        opts,
        files,
        footnotes,
        capture: None,
        code: None,
        image: None,
        table_cols: 0,
    };
    let events: Vec<Event> = Parser::new_ext(md, parser_opts).collect();
    for (i, ev) in events.iter().enumerate() {
        // A task list draws its boxes; it doesn't need bullets as well.
        let task_next = |j: usize| match events.get(j) {
            Some(Event::TaskListMarker(_)) => true,
            // A loose item's text sits in a paragraph.
            Some(Event::Start(Tag::Paragraph)) => {
                matches!(events.get(j + 1), Some(Event::TaskListMarker(_)))
            }
            _ => false,
        };
        if matches!(ev, Event::Start(Tag::List(None)))
            && matches!(events.get(i + 1), Some(Event::Start(Tag::Item)))
            && task_next(i + 2)
        {
            w.out.push_str("\n\n#list(marker: [], ");
            continue;
        }
        w.event(ev.clone());
    }
    w.out
}

/// The minimum Typst needs: one source, the fonts, the pictures.
struct DocWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main: Source,
    files: HashMap<FileId, Bytes>,
}

fn file_id(path: &str) -> FileId {
    FileId::new(RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new(path).expect("a valid virtual path"),
    ))
}

impl DocWorld {
    fn new(source: String, files: HashMap<String, Vec<u8>>, font_files: &[&'static [u8]]) -> Self {
        let fonts: Vec<Font> = font_files
            .iter()
            .flat_map(|f| Font::iter(Bytes::new(*f)))
            .collect();
        DocWorld {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(FontBook::from_fonts(&fonts)),
            fonts,
            main: Source::new(file_id("/main.typ"), source),
            files: files
                .into_iter()
                .map(|(p, b)| (file_id(&p), Bytes::new(b)))
                .collect(),
        }
    }
}

impl World for DocWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            Ok(self.main.clone())
        } else {
            Err(FileError::AccessDenied)
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.files.get(&id).cloned().ok_or(FileError::AccessDenied)
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _: Option<typst::foundations::Duration>) -> Option<Datetime> {
        None
    }
}

/// Keep `Arc` in the public API's vocabulary for callers building
/// `DrawOptions`.
pub type FontDb = Arc<mermaid::fontdb::Database>;

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        Options {
            style: Style::GitHub,
            numbered: false,
            letter: false,
            fonts: Vec::new(),
            base_dir: None,
            diagrams: None,
        }
    }

    #[test]
    fn text_never_becomes_typst_code() {
        let mut files = HashMap::new();
        let t = to_typst("Hi #set page(width: 1pt) \"q\" $5 [x]", &opts(), &mut files);
        // Every piece is a string literal: `#"…";`, nothing else.
        let rest = t
            .trim()
            .split("\";")
            .filter(|p| !p.is_empty())
            .all(|p| p.starts_with("#\""));
        assert!(rest, "{t}");
        assert!(t.contains("#set page(width: 1pt)"), "{t}");
    }

    fn fonts() -> Vec<&'static [u8]> {
        vec![
            include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
            include_bytes!("../../../assets/fonts/Inter-Bold.ttf"),
            include_bytes!("../../../assets/fonts/Inter-Italic.ttf"),
            include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
        ]
    }

    #[test]
    fn a_whole_document_typesets() {
        let o = Options {
            fonts: fonts(),
            ..opts()
        };
        let md = "# Report\n\nText with **bold**, `code`, $x^2$ and a [link](https://x.y).\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n- [ ] task\n\n> quote\n";
        let pdf = markdown_to_pdf(md, "Report", &o).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert!(pdf.len() > 2000);
    }

    #[test]
    fn structure_maps_to_typst_functions() {
        let mut files = HashMap::new();
        let md = "# Title\n\nSome **bold** and *it* and `code`.\n\n- a\n\n* [x] done\n\n1. one\n\n> [!NOTE]\n> careful\n\n| a | b |\n|:--|--:|\n| 1 | 2 |\n\n```rust\nfn x() {}\n```\n\nNote.[^1]\n\n[^1]: The footnote.\n";
        let t = to_typst(md, &opts(), &mut files);
        assert!(t.contains("#heading(level: 1)[#\"Title\";]"), "{t}");
        assert!(t.contains("#strong[#\"bold\";]"), "{t}");
        assert!(t.contains("#emph[#\"it\";]"), "{t}");
        assert!(t.contains("#raw(\"code\");"), "{t}");
        assert!(t.contains("#list(["), "{t}");
        assert!(t.contains("#task(true)"), "{t}");
        assert!(t.contains("#list(marker: [], [#task(true)"), "{t}");
        assert!(t.contains("#enum(start: 1, "), "{t}");
        assert!(t.contains("#alert(\"#0969da\", \"Note\")["), "{t}");
        assert!(
            t.contains("#table(columns: 2, align: (left, right,), table.header(repeat: true, "),
            "{t}"
        );
        assert!(
            t.contains("#raw(block: true, lang: \"rust\", \"fn x() {}\")"),
            "{t}"
        );
        assert!(t.contains("#footnote[#\"The footnote.\";]"), "{t}");
    }

    #[test]
    fn math_becomes_pictures() {
        let mut files = HashMap::new();
        let t = to_typst("Area $x^2$ and\n\n$$\\frac{1}{2}$$\n", &opts(), &mut files);
        assert_eq!(files.len(), 2);
        assert!(t.contains("#box(baseline:"), "{t}");
        assert!(t.contains("#align(center, image("), "{t}");
    }
}
