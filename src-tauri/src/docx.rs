//! Minimal OOXML (.docx) → GitHub-flavored markdown converter.
//!
//! Covers what survives a markdown round-trip: headings, bold/italic/
//! strikethrough, hyperlinks, nested bullet/numbered lists, tables, quote
//! styles, and line breaks. Images, footnotes, and field instructions
//! (TOC machinery etc.) are skipped; field *results* are kept as text.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use roxmltree::Node;

pub fn docx_to_markdown(bytes: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("not a docx: {e}"))?;
    let document = read_zip_file(&mut archive, "word/document.xml")
        .ok_or_else(|| "no word/document.xml — not a Word document".to_string())?;
    let rels = read_zip_file(&mut archive, "word/_rels/document.xml.rels").unwrap_or_default();
    let numbering = read_zip_file(&mut archive, "word/numbering.xml").unwrap_or_default();
    convert(&document, &rels, &numbering)
}

fn read_zip_file(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    let mut out = String::new();
    file.read_to_string(&mut out).ok()?;
    Some(out)
}

/// Relationship Id → external target URL.
fn parse_rels(xml: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return map;
    };
    for rel in doc.descendants().filter(|n| n.has_tag_name_local("Relationship")) {
        if let (Some(id), Some(target)) = (rel.attribute("Id"), rel.attribute("Target")) {
            map.insert(id.to_string(), target.to_string());
        }
    }
    map
}

/// numId → (ilvl → is_bullet).
fn parse_numbering(xml: &str) -> HashMap<String, HashMap<String, bool>> {
    let mut abstract_levels: HashMap<String, HashMap<String, bool>> = HashMap::new();
    let mut map = HashMap::new();
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return map;
    };
    for an in doc.descendants().filter(|n| n.has_tag_name_local("abstractNum")) {
        let Some(id) = an.attr_local("abstractNumId") else { continue };
        let mut levels = HashMap::new();
        for lvl in an.descendants().filter(|n| n.has_tag_name_local("lvl")) {
            let Some(ilvl) = lvl.attr_local("ilvl") else { continue };
            let fmt = lvl
                .children()
                .find(|n| n.has_tag_name_local("numFmt"))
                .and_then(|n| n.attr_local("val"))
                .unwrap_or("bullet");
            levels.insert(ilvl.to_string(), fmt == "bullet");
        }
        abstract_levels.insert(id.to_string(), levels);
    }
    for num in doc.descendants().filter(|n| n.has_tag_name_local("num")) {
        let Some(num_id) = num.attr_local("numId") else { continue };
        let Some(abs) = num
            .children()
            .find(|n| n.has_tag_name_local("abstractNumId"))
            .and_then(|n| n.attr_local("val"))
        else {
            continue;
        };
        if let Some(levels) = abstract_levels.get(abs) {
            map.insert(num_id.to_string(), levels.clone());
        }
    }
    map
}

trait NodeExt<'a> {
    fn has_tag_name_local(&self, name: &str) -> bool;
    fn attr_local(&self, name: &str) -> Option<&'a str>;
}

impl<'a, 'input: 'a> NodeExt<'a> for Node<'a, 'input> {
    fn has_tag_name_local(&self, name: &str) -> bool {
        self.is_element() && self.tag_name().name() == name
    }
    fn attr_local(&self, name: &str) -> Option<&'a str> {
        self.attributes()
            .find(|a| a.name() == name)
            .map(|a| a.value())
    }
}

#[derive(PartialEq, Clone, Copy)]
enum BlockKind {
    Normal,
    ListItem,
    Quote,
}

struct Ctx {
    rels: HashMap<String, String>,
    numbering: HashMap<String, HashMap<String, bool>>,
}

fn convert(document: &str, rels: &str, numbering: &str) -> Result<String, String> {
    let doc =
        roxmltree::Document::parse(document).map_err(|e| format!("bad document.xml: {e}"))?;
    let ctx = Ctx {
        rels: parse_rels(rels),
        numbering: parse_numbering(numbering),
    };
    let Some(body) = doc.descendants().find(|n| n.has_tag_name_local("body")) else {
        return Ok(String::new());
    };
    let mut blocks: Vec<(BlockKind, String)> = Vec::new();
    for child in body.children() {
        if child.has_tag_name_local("p") {
            if let Some(b) = paragraph_to_md(&child, &ctx) {
                blocks.push(b);
            }
        } else if child.has_tag_name_local("tbl") {
            let t = table_to_md(&child, &ctx);
            if !t.is_empty() {
                blocks.push((BlockKind::Normal, t));
            }
        }
    }
    let mut out = String::new();
    let mut prev: Option<BlockKind> = None;
    for (kind, text) in blocks {
        match prev {
            None => {}
            // Adjacent list items stay tight; adjacent quotes merge into
            // one blockquote with a paragraph break.
            Some(BlockKind::ListItem) if kind == BlockKind::ListItem => out.push('\n'),
            Some(BlockKind::Quote) if kind == BlockKind::Quote => out.push_str("\n>\n"),
            Some(_) => out.push_str("\n\n"),
        }
        out.push_str(&text);
        prev = Some(kind);
    }
    let out = out.trim().to_string();
    Ok(if out.is_empty() {
        out
    } else {
        format!("{out}\n")
    })
}

fn style_of(p: &Node) -> Option<String> {
    p.children()
        .find(|n| n.has_tag_name_local("pPr"))?
        .children()
        .find(|n| n.has_tag_name_local("pStyle"))?
        .attr_local("val")
        .map(|s| s.to_string())
}

/// (ilvl, is_bullet) when the paragraph is a list item.
fn list_info(p: &Node, ctx: &Ctx) -> Option<(usize, bool)> {
    let num_pr = p
        .children()
        .find(|n| n.has_tag_name_local("pPr"))?
        .children()
        .find(|n| n.has_tag_name_local("numPr"))?;
    let ilvl = num_pr
        .children()
        .find(|n| n.has_tag_name_local("ilvl"))
        .and_then(|n| n.attr_local("val"))
        .unwrap_or("0");
    let num_id = num_pr
        .children()
        .find(|n| n.has_tag_name_local("numId"))
        .and_then(|n| n.attr_local("val"))?;
    if num_id == "0" {
        return None; // numId 0 = "not numbered"
    }
    let bullet = ctx
        .numbering
        .get(num_id)
        .and_then(|levels| levels.get(ilvl))
        .copied()
        .unwrap_or(true);
    Some((ilvl.parse().unwrap_or(0), bullet))
}

fn paragraph_to_md(p: &Node, ctx: &Ctx) -> Option<(BlockKind, String)> {
    let text = runs_to_md(p, ctx);
    let text = text.trim();
    if let Some((ilvl, bullet)) = list_info(p, ctx) {
        if text.is_empty() {
            return None;
        }
        let indent = "    ".repeat(ilvl);
        let marker = if bullet { "- " } else { "1. " };
        return Some((BlockKind::ListItem, format!("{indent}{marker}{text}")));
    }
    let style = style_of(p).unwrap_or_default();
    let heading = match style.as_str() {
        "Title" | "Heading1" => 1,
        "Subtitle" | "Heading2" => 2,
        "Heading3" => 3,
        "Heading4" => 4,
        "Heading5" => 5,
        "Heading6" | "Heading7" | "Heading8" | "Heading9" => 6,
        _ => 0,
    };
    if text.is_empty() {
        return None;
    }
    if heading > 0 {
        let flat = text.replace("\\\n", " ");
        return Some((BlockKind::Normal, format!("{} {flat}", "#".repeat(heading))));
    }
    if matches!(style.as_str(), "Quote" | "IntenseQuote" | "BlockQuote" | "BlockQuotation") {
        let quoted = text
            .lines()
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Some((BlockKind::Quote, quoted));
    }
    Some((BlockKind::Normal, text.to_string()))
}

/// Inline content of a paragraph (or table cell paragraph).
fn runs_to_md(node: &Node, ctx: &Ctx) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.has_tag_name_local("r") {
            out.push_str(&run_to_md(&child));
        } else if child.has_tag_name_local("hyperlink") {
            let inner: String = child
                .children()
                .filter(|n| n.has_tag_name_local("r"))
                .map(|r| run_to_md(&r))
                .collect();
            let url = child
                .attr_local("id")
                .and_then(|id| ctx.rels.get(id))
                .cloned()
                .or_else(|| child.attr_local("anchor").map(|a| format!("#{a}")));
            match url {
                Some(u) if !inner.trim().is_empty() => {
                    out.push_str(&format!("[{}]({})", inner.trim(), u));
                }
                _ => out.push_str(&inner),
            }
        } else if child.has_tag_name_local("ins")
            || child.has_tag_name_local("smartTag")
            || child.has_tag_name_local("sdt")
            || child.has_tag_name_local("sdtContent")
        {
            out.push_str(&runs_to_md(&child, ctx));
        }
        // w:del (deleted revisions), bookmarks, proofErr: skipped
    }
    out
}

fn run_to_md(r: &Node) -> String {
    let pr = r.children().find(|n| n.has_tag_name_local("rPr"));
    let has = |name: &str| -> bool {
        pr.map(|pr| {
            pr.children().any(|n| {
                n.has_tag_name_local(name)
                    && n.attr_local("val").map(|v| v != "0" && v != "false").unwrap_or(true)
            })
        })
        .unwrap_or(false)
    };
    let mut text = String::new();
    for child in r.children() {
        if child.has_tag_name_local("t") {
            text.push_str(&escape_md(child.text().unwrap_or_default()));
        } else if child.has_tag_name_local("br") || child.has_tag_name_local("cr") {
            text.push_str("\\\n");
        } else if child.has_tag_name_local("tab") {
            text.push(' ');
        } else if child.has_tag_name_local("noBreakHyphen") {
            text.push('-');
        }
        // w:instrText (field instructions like TOC/HYPERLINK codes),
        // w:drawing, w:pict (images): skipped
    }
    if text.trim().is_empty() {
        return text;
    }
    // Word marks code with monospace fonts, not a run property; the closest
    // portable signal is the "HTMLCode"/"CodeChar" character styles.
    let code_style = pr
        .and_then(|pr| pr.children().find(|n| n.has_tag_name_local("rStyle")))
        .and_then(|s| s.attr_local("val").map(|v| v.contains("Code")))
        .unwrap_or(false);
    if code_style {
        return format!("`{}`", text.trim());
    }
    if has("strike") || has("dstrike") {
        text = wrap_trimmed(&text, "~~");
    }
    if has("i") || has("iCs") {
        text = wrap_trimmed(&text, "*");
    }
    if has("b") || has("bCs") {
        text = wrap_trimmed(&text, "**");
    }
    text
}

fn wrap_trimmed(s: &str, mark: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return s.to_string();
    }
    let lead = &s[..s.len() - s.trim_start().len()];
    let trail = &s[s.trim_end().len()..];
    format!("{lead}{mark}{trimmed}{mark}{trail}")
}

fn table_to_md(tbl: &Node, ctx: &Ctx) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for tr in tbl.children().filter(|n| n.has_tag_name_local("tr")) {
        let mut row = Vec::new();
        for tc in tr.children().filter(|n| n.has_tag_name_local("tc")) {
            let cell: Vec<String> = tc
                .children()
                .filter(|n| n.has_tag_name_local("p"))
                .map(|p| runs_to_md(&p, ctx).trim().replace("\\\n", " "))
                .filter(|s| !s.is_empty())
                .collect();
            row.push(cell.join(" ").replace('|', "\\|").replace('\n', " "));
        }
        if !row.is_empty() {
            rows.push(row);
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        let mut cells = row.clone();
        cells.resize(cols, String::new());
        out.push_str("| ");
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
        if i == 0 {
            out.push('|');
            out.push_str(&" --- |".repeat(cols));
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

fn escape_md(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '\u{a0}' {
            out.push(' ');
            continue;
        }
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '~' | '<' | '&' | '#' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    fn doc(body: &str) -> String {
        format!(r#"<?xml version="1.0"?><w:document {W}><w:body>{body}</w:body></w:document>"#)
    }

    #[test]
    fn headings_and_emphasis() {
        let d = doc(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>
               <w:p><w:r><w:t>plain </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r><w:r><w:rPr><w:i/><w:strike/></w:rPr><w:t> both</w:t></w:r></w:p>"#,
        );
        let md = convert(&d, "", "").unwrap();
        assert_eq!(md, "# Title\n\nplain **bold** *~~both~~*\n");
    }

    #[test]
    fn hyperlinks_resolve_rels() {
        let d = doc(
            r#"<w:p><w:hyperlink r:id="rId4"><w:r><w:t>site</w:t></w:r></w:hyperlink></w:p>"#,
        );
        let rels = r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId4" Target="https://example.com" TargetMode="External"/></Relationships>"#;
        let md = convert(&d, rels, "").unwrap();
        assert_eq!(md, "[site](https://example.com)\n");
    }

    #[test]
    fn nested_lists_via_numbering() {
        let d = doc(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>one</w:t></w:r></w:p>
               <w:p><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>nested</w:t></w:r></w:p>
               <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr><w:r><w:t>first</w:t></w:r></w:p>"#,
        );
        let numbering = format!(
            r#"<?xml version="1.0"?><w:numbering {W}>
              <w:abstractNum w:abstractNumId="10"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl><w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
              <w:abstractNum w:abstractNumId="11"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
              <w:num w:numId="1"><w:abstractNumId w:val="10"/></w:num>
              <w:num w:numId="2"><w:abstractNumId w:val="11"/></w:num>
            </w:numbering>"#
        );
        let md = convert(&d, "", &numbering).unwrap();
        assert_eq!(md, "- one\n    - nested\n1. first\n");
    }

    #[test]
    fn tables_and_quotes() {
        let d = doc(
            r#"<w:tbl>
                 <w:tr><w:tc><w:p><w:r><w:t>H1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>H2</w:t></w:r></w:p></w:tc></w:tr>
                 <w:tr><w:tc><w:p><w:r><w:t>a|b</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr>
               </w:tbl>
               <w:p><w:pPr><w:pStyle w:val="Quote"/></w:pPr><w:r><w:t>wise words</w:t></w:r></w:p>
               <w:p><w:pPr><w:pStyle w:val="Quote"/></w:pPr><w:r><w:t>more words</w:t></w:r></w:p>"#,
        );
        let md = convert(&d, "", "").unwrap();
        assert_eq!(
            md,
            "| H1 | H2 |\n| --- | --- |\n| a\\|b | c |\n\n> wise words\n>\n> more words\n"
        );
    }

    #[test]
    fn field_instructions_skipped_breaks_kept() {
        let d = doc(
            r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>TOC \o "1-3"</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>kept result</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>
               <w:p><w:r><w:t>line</w:t><w:br/><w:t>two</w:t></w:r></w:p>"#,
        );
        let md = convert(&d, "", "").unwrap();
        assert_eq!(md, "kept result\n\nline\\\ntwo\n");
    }

    #[test]
    fn full_zip_roundtrip() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("word/document.xml", opts).unwrap();
            zw.write_all(doc(r#"<w:p><w:r><w:t>hello docx</w:t></w:r></w:p>"#).as_bytes())
                .unwrap();
            zw.finish().unwrap();
        }
        assert_eq!(docx_to_markdown(&buf).unwrap(), "hello docx\n");
    }
}
