use pulldown_cmark::{html, Options, Parser};

/// Render GitHub-flavored markdown to HTML.
///
/// Extensions match GitHub's: tables, task lists, strikethrough, footnotes,
/// and alerts (`> [!NOTE]` etc., via ENABLE_GFM).
pub fn to_html(md: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_GFM);
    let parser = Parser::new_ext(md, opts);
    let mut out = String::new();
    html::push_html(&mut out, parser);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_gfm_extensions() {
        let html = to_html(
            "| a |\n|---|\n| b |\n\n- [x] task\n\n~~gone~~\n\nfoot[^1]\n\n[^1]: note\n\n> [!NOTE]\n> alert",
        );
        assert!(html.contains("<table>"));
        assert!(html.contains("type=\"checkbox\""));
        assert!(html.contains("<del>"));
        assert!(html.contains("footnote-reference"));
        assert!(html.contains("markdown-alert-note"));
    }
}
