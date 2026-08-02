//! Serialize the WYSIWYG contenteditable DOM back into GitHub-flavored
//! markdown. Handles everything `markdown::to_html` can produce (headings,
//! emphasis, code, links, images, lists, task lists, tables, blockquotes,
//! GFM alerts, footnotes, hr) plus the tags contenteditable editing inserts
//! (`<div>` paragraphs, `<b>`/`<i>`, style spans).

use wasm_bindgen::JsCast;
use web_sys::{Element, HtmlInputElement, Node};

pub fn dom_to_markdown(root: &Element) -> String {
    let node: &Node = root.unchecked_ref();
    let blocks = serialize_blocks(node, &|_| false);
    let mut out = blocks.join("\n\n");
    out = out.trim().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn children(node: &Node) -> Vec<Node> {
    let list = node.child_nodes();
    (0..list.length()).filter_map(|i| list.item(i)).collect()
}

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "H1" | "H2"
            | "H3"
            | "H4"
            | "H5"
            | "H6"
            | "P"
            | "DIV"
            | "UL"
            | "OL"
            | "PRE"
            | "TABLE"
            | "BLOCKQUOTE"
            | "HR"
            | "SECTION"
            | "ARTICLE"
            | "FIGURE"
    )
}

fn has_block_child(node: &Node) -> bool {
    children(node).iter().any(|c| {
        c.dyn_ref::<Element>()
            .map(|el| is_block_tag(&el.tag_name().to_uppercase()))
            .unwrap_or(false)
    })
}

/// Walk `node`'s children as a sequence of blocks. Inline content between
/// block elements is gathered into implicit paragraphs. `skip` filters out
/// elements the caller handles itself (task checkboxes, footnote labels).
fn serialize_blocks(node: &Node, skip: &dyn Fn(&Element) -> bool) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut run = String::new();

    let flush = |run: &mut String, blocks: &mut Vec<String>| {
        let text = trim_hard_breaks(run.trim());
        if !text.is_empty() {
            blocks.push(fix_line_starts(&text));
        }
        run.clear();
    };

    for child in children(node) {
        if let Some(el) = child.dyn_ref::<Element>() {
            if skip(el) {
                continue;
            }
            let tag = el.tag_name().to_uppercase();
            if is_block_tag(&tag) {
                flush(&mut run, &mut blocks);
                let b = block_to_md(el, &tag);
                if !b.trim().is_empty() {
                    blocks.push(b);
                }
                continue;
            }
        }
        run.push_str(&inline(&child));
    }
    flush(&mut run, &mut blocks);
    blocks
}

fn block_to_md(el: &Element, tag: &str) -> String {
    match tag {
        "H1" | "H2" | "H3" | "H4" | "H5" | "H6" => {
            let level = tag[1..].parse::<usize>().unwrap_or(1);
            let text = inline_children(el);
            format!("{} {}", "#".repeat(level), text.trim().replace('\n', " "))
        }
        "PRE" => pre_to_md(el),
        "BLOCKQUOTE" => blockquote_to_md(el),
        "UL" | "OL" => list_to_md(el, tag == "OL"),
        "TABLE" => table_to_md(el),
        "HR" => "---".to_string(),
        "DIV" if el.class_list().contains("footnote-definition") => footnote_def_to_md(el),
        "P" | "DIV" | "SECTION" | "ARTICLE" | "FIGURE" => {
            if has_block_child(el.unchecked_ref()) {
                serialize_blocks(el.unchecked_ref(), &|_| false).join("\n\n")
            } else {
                let text = trim_hard_breaks(inline_children(el).trim());
                fix_line_starts(&text)
            }
        }
        _ => serialize_blocks(el.unchecked_ref(), &|_| false).join("\n\n"),
    }
}

fn pre_to_md(el: &Element) -> String {
    let mut lang = String::new();
    if let Some(code) = el.query_selector("code").ok().flatten() {
        let classes = code.class_name();
        for cls in classes.split_whitespace() {
            if let Some(l) = cls.strip_prefix("language-") {
                lang = l.to_string();
                break;
            }
        }
    }
    let text = el.text_content().unwrap_or_default();
    let text = text.trim_end_matches('\n');
    let fence = if text.contains("```") { "````" } else { "```" };
    format!("{fence}{lang}\n{text}\n{fence}")
}

fn blockquote_to_md(el: &Element) -> String {
    let mut inner_blocks = serialize_blocks(el.unchecked_ref(), &|_| false);
    // GFM alerts render as <blockquote class="markdown-alert-note"> etc.
    // The [!KIND] marker must sit directly above the first content line.
    let classes = el.class_name();
    for kind in ["note", "tip", "important", "warning", "caution"] {
        if classes.contains(&format!("markdown-alert-{kind}")) {
            let marker = format!("[!{}]", kind.to_uppercase());
            if inner_blocks.is_empty() {
                inner_blocks.push(marker);
            } else {
                inner_blocks[0] = format!("{marker}\n{}", inner_blocks[0]);
            }
            break;
        }
    }
    let inner = inner_blocks.join("\n\n");
    inner
        .lines()
        .map(|l| {
            if l.is_empty() {
                ">".to_string()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn checkbox_state(li: &Element) -> Option<bool> {
    for child in children(li.unchecked_ref()) {
        if let Some(input) = child.dyn_ref::<HtmlInputElement>() {
            if input.type_() == "checkbox" {
                return Some(input.checked());
            }
        }
    }
    None
}

fn list_to_md(el: &Element, ordered: bool) -> String {
    let start: u64 = el
        .get_attribute("start")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let mut items: Vec<String> = Vec::new();
    let mut n = start;
    for child in children(el.unchecked_ref()) {
        let Some(li) = child.dyn_ref::<Element>() else {
            continue;
        };
        if li.tag_name().to_uppercase() != "LI" {
            continue;
        }
        let marker = if ordered {
            format!("{n}. ")
        } else {
            "- ".to_string()
        };
        n += 1;
        let task = checkbox_state(li);
        let task_prefix = match task {
            Some(true) => "[x] ",
            Some(false) => "[ ] ",
            None => "",
        };
        let blocks = serialize_blocks(li.unchecked_ref(), &|e| {
            e.dyn_ref::<HtmlInputElement>()
                .map(|i| i.type_() == "checkbox")
                .unwrap_or(false)
        });
        items.push(assemble_list_item(&marker, task_prefix, &blocks));
    }
    items.join("\n")
}

fn assemble_list_item(marker: &str, task_prefix: &str, blocks: &[String]) -> String {
    let indent = " ".repeat(marker.len());
    if blocks.is_empty() {
        return format!("{marker}{task_prefix}");
    }
    let mut out = String::new();
    for (bi, block) in blocks.iter().enumerate() {
        if bi > 0 {
            // Nested lists attach directly; other blocks need a blank line.
            out.push_str(if is_list_block(block) { "\n" } else { "\n\n" });
        }
        for (li_line, line) in block.lines().enumerate() {
            if li_line > 0 {
                out.push('\n');
            }
            if bi == 0 && li_line == 0 {
                out.push_str(marker);
                out.push_str(task_prefix);
                out.push_str(line);
            } else {
                out.push_str(&indent);
                out.push_str(line);
            }
        }
    }
    out
}

fn is_list_block(block: &str) -> bool {
    let first = block.lines().next().unwrap_or("");
    let t = first.trim_start();
    t.starts_with("- ")
        || t.starts_with("* ")
        || t.chars()
            .take_while(|c| c.is_ascii_digit())
            .count()
            .checked_sub(0)
            .map(|d| {
                d > 0
                    && t.chars()
                        .nth(d)
                        .map(|c| c == '.' || c == ')')
                        .unwrap_or(false)
            })
            .unwrap_or(false)
}

fn cell_alignment(cell: &Element) -> char {
    let style = cell.get_attribute("style").unwrap_or_default();
    let align = cell.get_attribute("align").unwrap_or_default();
    let s = format!("{style} {align}");
    if s.contains("center") {
        'c'
    } else if s.contains("right") {
        'r'
    } else if s.contains("left") {
        'l'
    } else {
        '-'
    }
}

fn cell_text(cell: &Element) -> String {
    inline_children(cell)
        .trim()
        .replace('\n', " ")
        .replace('|', "\\|")
}

fn table_to_md(el: &Element) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut aligns: Vec<char> = Vec::new();
    if let Ok(cells) = el.query_selector_all("tr") {
        for i in 0..cells.length() {
            let Some(tr) = cells.item(i) else { continue };
            let mut row: Vec<String> = Vec::new();
            for cell in children(&tr) {
                let Some(c) = cell.dyn_ref::<Element>() else {
                    continue;
                };
                let tag = c.tag_name().to_uppercase();
                if tag == "TD" || tag == "TH" {
                    if rows.is_empty() {
                        aligns.push(cell_alignment(c));
                    }
                    row.push(cell_text(c));
                }
            }
            if !row.is_empty() {
                rows.push(row);
            }
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
    aligns.resize(cols, '-');
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        let mut cells = row.clone();
        cells.resize(cols, String::new());
        out.push_str("| ");
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
        if i == 0 {
            out.push('|');
            for a in &aligns {
                out.push_str(match a {
                    'c' => " :---: |",
                    'r' => " ---: |",
                    'l' => " :--- |",
                    _ => " --- |",
                });
            }
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

fn footnote_def_to_md(el: &Element) -> String {
    let name = el.id();
    let blocks = serialize_blocks(el.unchecked_ref(), &|e| {
        e.tag_name().to_uppercase() == "SUP"
            && e.class_list().contains("footnote-definition-label")
    });
    let body = blocks.join("\n\n");
    let mut out = String::new();
    for (i, line) in body.lines().enumerate() {
        if i == 0 {
            out.push_str(&format!("[^{name}]: {line}"));
        } else {
            out.push('\n');
            out.push_str("    ");
            out.push_str(line);
        }
    }
    if out.is_empty() {
        out = format!("[^{name}]:");
    }
    out
}

fn inline_children(el: &Element) -> String {
    children(el.unchecked_ref())
        .iter()
        .map(inline)
        .collect::<String>()
}

fn wrap_nonempty(inner: &str, mark: &str) -> String {
    // Emphasis markers don't tolerate leading/trailing spaces inside them.
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return inner.to_string();
    }
    let lead = &inner[..inner.len() - inner.trim_start().len()];
    let trail = &inner[inner.trim_end().len()..];
    format!("{lead}{mark}{trimmed}{mark}{trail}")
}

fn code_span(text: &str) -> String {
    if text.contains('`') {
        format!("`` {text} ``")
    } else {
        format!("`{text}`")
    }
}

fn inline(node: &Node) -> String {
    if node.node_type() == Node::TEXT_NODE {
        return escape_text(&node.text_content().unwrap_or_default());
    }
    let Some(el) = node.dyn_ref::<Element>() else {
        return String::new();
    };
    let tag = el.tag_name().to_uppercase();
    match tag.as_str() {
        "BR" => "\\\n".to_string(),
        "STRONG" | "B" => wrap_nonempty(&inline_children(el), "**"),
        "EM" | "I" => wrap_nonempty(&inline_children(el), "*"),
        "DEL" | "S" | "STRIKE" => wrap_nonempty(&inline_children(el), "~~"),
        "CODE" => code_span(&el.text_content().unwrap_or_default()),
        "A" => {
            let text = inline_children(el);
            let href = el.get_attribute("href").unwrap_or_default();
            if href.is_empty() {
                text
            } else {
                match el.get_attribute("title") {
                    Some(t) if !t.is_empty() => format!("[{text}]({href} \"{t}\")"),
                    _ => format!("[{text}]({href})"),
                }
            }
        }
        "IMG" => {
            let alt = el.get_attribute("alt").unwrap_or_default();
            let src = el.get_attribute("src").unwrap_or_default();
            match el.get_attribute("title") {
                Some(t) if !t.is_empty() => format!("![{alt}]({src} \"{t}\")"),
                _ => format!("![{alt}]({src})"),
            }
        }
        "SUP" if el.class_list().contains("footnote-reference") => {
            let name = el
                .query_selector("a")
                .ok()
                .flatten()
                .and_then(|a| a.get_attribute("href"))
                .map(|h| h.trim_start_matches('#').to_string())
                .unwrap_or_else(|| el.text_content().unwrap_or_default());
            format!("[^{name}]")
        }
        "INPUT" => String::new(),
        "SPAN" | "FONT" => {
            // contenteditable sometimes styles with spans instead of tags.
            let inner = inline_children(el);
            let style = el.get_attribute("style").unwrap_or_default();
            let mut out = inner;
            if style.contains("line-through") {
                out = wrap_nonempty(&out, "~~");
            }
            if style.contains("font-style: italic") || style.contains("font-style:italic") {
                out = wrap_nonempty(&out, "*");
            }
            if style.contains("font-weight: bold")
                || style.contains("font-weight:bold")
                || style.contains("font-weight: 700")
            {
                out = wrap_nonempty(&out, "**");
            }
            out
        }
        _ => inline_children(el),
    }
}

/// Escape characters that are markdown syntax anywhere in inline text.
/// Line-start-only syntax (#, >, -, digits) is handled by `fix_line_starts`.
fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '~' | '<' | '&') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Remove trailing hard-break markers (a `<br>` at the very end of a
/// paragraph, which contenteditable likes to leave behind).
fn trim_hard_breaks(s: &str) -> String {
    let mut t = s.trim_end();
    while t.ends_with('\\') {
        t = t[..t.len() - 1].trim_end();
    }
    t.to_string()
}

/// Escape characters that would start a block construct at line start.
fn fix_line_starts(block: &str) -> String {
    block
        .lines()
        .map(|line| {
            let ws_len = line.len() - line.trim_start().len();
            let (ws, rest) = line.split_at(ws_len);
            let mut fixed = String::new();
            if rest.starts_with('#') || rest.starts_with('>') || rest.starts_with("+ ") {
                fixed = format!("\\{rest}");
            } else if rest.starts_with("- ") || rest == "-" {
                fixed = format!("\\{rest}");
            } else {
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    let after = &rest[digits.len()..];
                    if after.starts_with(". ") || after.starts_with(") ") {
                        fixed = format!("{digits}\\{after}");
                    }
                }
            }
            if fixed.is_empty() {
                format!("{ws}{rest}")
            } else {
                format!("{ws}{fixed}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
