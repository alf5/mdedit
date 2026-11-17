//! Spelling (and, if asked, grammar) with Harper, which is English only.
//! Code, formulas, wiki link targets, tags, URLs and frontmatter are left
//! alone. Harper counts in characters; what comes out is in bytes.

use std::ops::Range;
use std::sync::Arc;

use harper_core::linting::{LintGroup, LintKind, Linter as _, Suggestion};
use harper_core::spell::{FstDictionary, MergedDictionary, MutableDictionary};
use harper_core::{Dialect, DictWordMetadata, Document};

use crate::syntax::{LineKind, Syntax, style};

/// Something to underline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub range: Range<usize>,
    pub message: String,
    /// What could replace `range`, best first.
    pub suggestions: Vec<String>,
    /// A misspelling (red), rather than grammar or style (blue).
    pub spelling: bool,
}

/// The user's own words, one a line, beside the settings.
pub fn words_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("mdedit").join("dictionary.txt"))
}

/// The words added to the dictionary.
pub fn known_words() -> Vec<String> {
    words_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Add `word` to the user's dictionary.
pub fn learn(word: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let path = words_path().ok_or_else(|| std::io::Error::other("no config folder"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{}", word.trim())
}

/// The English the dictionary follows.
pub fn dialect(name: &str) -> Dialect {
    match name {
        "british" => Dialect::British,
        "canadian" => Dialect::Canadian,
        "australian" => Dialect::Australian,
        "indian" => Dialect::Indian,
        _ => Dialect::American,
    }
}

/// The issues in `text`: misspellings, and grammar too when `grammar` is
/// set. Words in `known` are spelled right.
pub fn check(
    text: &str,
    syntax: &Syntax,
    grammar: bool,
    dialect: Dialect,
    known: &[String],
) -> Vec<Issue> {
    let mut mine = MutableDictionary::new();
    for w in known {
        mine.append_word_str(w, DictWordMetadata::default());
    }
    let mut dict = MergedDictionary::new();
    dict.add_dictionary(FstDictionary::curated());
    dict.add_dictionary(Arc::new(mine));
    let dict = Arc::new(dict);
    let mut lints = LintGroup::new_curated(dict.clone(), dialect);
    if !grammar {
        lints.set_all_rules_to(Some(false));
        lints.config.set_rule_enabled("SpellCheck", true);
    }
    let doc = Document::new_markdown_default(text, dict.as_ref());

    // Character index → byte offset.
    let bytes: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .chain([text.len()])
        .collect();
    let skipped = |r: &Range<usize>| {
        let line = &syntax.lines[syntax.line_at(r.start)];
        matches!(
            line.kind,
            LineKind::Code
                | LineKind::FenceOpen
                | LineKind::FenceClose
                | LineKind::Meta
                | LineKind::Html
        ) || syntax.spans.iter().any(|s| {
            s.range.start < r.end
                && r.start < s.range.end
                && s.style & (style::CODE | style::MATH | style::WIKI | style::TAG) != 0
        }) || syntax.links.iter().any(|l| {
            !l.wiki
                && l.url.contains("://")
                && text[l.range.clone()].contains("://")
                && l.range.start < r.end
                && r.start < l.range.end
        })
    };
    let mut out: Vec<Issue> = lints
        .lint(&doc)
        .into_iter()
        .filter_map(|lint| {
            let start = *bytes.get(lint.span.start)?;
            let end = *bytes.get(lint.span.end)?;
            let range = start..end;
            if range.is_empty() || skipped(&range) {
                return None;
            }
            let span_chars: Vec<char> = text[range.clone()].chars().collect();
            let suggestions = lint
                .suggestions
                .iter()
                .map(|s| {
                    let mut chars = span_chars.clone();
                    s.apply(harper_core::Span::new(0, chars.len()), &mut chars);
                    match s {
                        Suggestion::Remove => String::new(),
                        _ => chars.into_iter().collect(),
                    }
                })
                .take(5)
                .collect();
            Some(Issue {
                range,
                message: lint.message,
                suggestions,
                spelling: lint.lint_kind == LintKind::Spelling,
            })
        })
        .collect();
    out.sort_by_key(|i| (i.range.start, i.range.end));
    out.dedup_by(|a, b| a.range == b.range);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    fn issues(text: &str, known: &[&str]) -> Vec<(String, Vec<String>)> {
        let known: Vec<String> = known.iter().map(|s| s.to_string()).collect();
        check(text, &parse(text), false, Dialect::American, &known)
            .into_iter()
            .map(|i| (text[i.range].to_string(), i.suggestions))
            .collect()
    }

    #[test]
    fn misspellings_are_found_in_bytes() {
        let found = issues("Café owners recieve visitors.\n", &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "recieve");
        assert!(found[0].1.contains(&"receive".to_string()), "{found:?}");
    }

    #[test]
    fn code_maths_links_and_tags_are_left_alone() {
        let text = "Run `cargoo bild` and $\\fracc{1}{2}$ with #mytagg and [[Notte]].\n\n```\nwrongg\n```\n";
        assert_eq!(issues(text, &[]), vec![]);
    }

    #[test]
    fn known_words_pass() {
        assert_eq!(issues("A gpuikit app.\n", &["gpuikit"]), vec![]);
        assert_eq!(issues("A gpuikit app.\n", &[]).len(), 1);
    }
}
