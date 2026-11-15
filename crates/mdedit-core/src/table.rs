//! Table source text: column padding, saved column widths and sort order.
//!
//! No UI in here, so all of it runs under a plain `cargo test`. The design,
//! and the `<!-- zen:cols=… -->` width comment, come from ZenNotes (MIT), so a
//! table resized in either app keeps its widths in the other.

use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    None,
    Left,
    Center,
    Right,
}

impl Align {
    /// Read the alignment off a cell's `style` and `align` attributes.
    pub fn from_attrs(attrs: &str) -> Self {
        if attrs.contains("center") {
            Align::Center
        } else if attrs.contains("right") {
            Align::Right
        } else if attrs.contains("left") {
            Align::Left
        } else {
            Align::None
        }
    }
}

/// A width in terminal columns, so CJK and emoji count as two and a padded
/// table still lines up when the file is read as text.
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn pad_cell(text: &str, width: usize, align: Align) -> String {
    let pad = width.saturating_sub(display_width(text));
    match align {
        Align::Right => format!("{}{text}", " ".repeat(pad)),
        Align::Center => {
            let left = pad / 2;
            format!("{}{text}{}", " ".repeat(left), " ".repeat(pad - left))
        }
        Align::None | Align::Left => format!("{text}{}", " ".repeat(pad)),
    }
}

fn delimiter_cell(width: usize, align: Align) -> String {
    match align {
        Align::None => "-".repeat(width),
        Align::Left => format!(":{}", "-".repeat(width.saturating_sub(1).max(1))),
        Align::Right => format!("{}:", "-".repeat(width.saturating_sub(1).max(1))),
        Align::Center => format!(":{}:", "-".repeat(width.saturating_sub(2).max(1))),
    }
}

/// Write a table as padded pipe text. `rows[0]` is the header. Cells must
/// already be markdown, with any `|` escaped. Short rows are filled out to the
/// widest one, and missing alignments count as none.
pub fn format_table(rows: &[Vec<String>], aligns: &[Align], widths: &[Option<u32>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let align_of = |c: usize| aligns.get(c).copied().unwrap_or(Align::None);
    let col_widths: Vec<usize> = (0..cols)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| display_width(s))
                .max()
                .unwrap_or(0)
                .max(3)
        })
        .collect();

    let render_row = |row: &Vec<String>| {
        let cells: Vec<String> = (0..cols)
            .map(|c| {
                let text = row.get(c).map_or("", String::as_str);
                pad_cell(text, col_widths[c], align_of(c))
            })
            .collect();
        format!("| {} |", cells.join(" | "))
    };

    let mut lines = Vec::with_capacity(rows.len() + 2);
    lines.push(render_row(&rows[0]));
    let delim: Vec<String> = (0..cols)
        .map(|c| delimiter_cell(col_widths[c], align_of(c)))
        .collect();
    lines.push(format!("| {} |", delim.join(" | ")));
    lines.extend(rows[1..].iter().map(render_row));
    if let Some(comment) = cols_comment(widths) {
        lines.push(comment);
    }
    lines.join("\n")
}

/// `<!-- zen:cols=120,auto,90 -->` to one entry per column, where `None` means
/// auto. Returns `None` for anything else, which leaves that comment alone.
pub fn parse_cols_comment(text: &str) -> Option<Vec<Option<u32>>> {
    let inner = text
        .trim()
        .strip_prefix("<!--")?
        .strip_suffix("-->")?
        .trim()
        .strip_prefix("zen:cols=")?
        .trim();
    let mut out = Vec::new();
    for part in inner.split(',') {
        let part = part.trim();
        if part.is_empty() || part.eq_ignore_ascii_case("auto") {
            out.push(None);
        } else {
            out.push(Some(part.parse::<u32>().ok().filter(|n| *n > 0)).flatten());
        }
    }
    Some(out)
}

/// The comment for these widths, or `None` when no column has one, so a table
/// nobody resized adds nothing to the file.
pub fn cols_comment(widths: &[Option<u32>]) -> Option<String> {
    if !widths.iter().any(Option::is_some) {
        return None;
    }
    let parts: Vec<String> = widths
        .iter()
        .map(|w| w.map_or_else(|| "auto".to_string(), |n| n.to_string()))
        .collect();
    Some(format!("<!-- zen:cols={} -->", parts.join(",")))
}

/// The `<colgroup>` markup for saved widths.
pub fn colgroup_html(widths: &[Option<u32>]) -> String {
    let mut out = String::from("<colgroup>");
    for w in widths {
        match w {
            Some(n) => out.push_str(&format!("<col style=\"width: {n}px\">")),
            None => out.push_str("<col>"),
        }
    }
    out.push_str("</colgroup>");
    out
}

/// The order to put rows in when sorting by `values`, one per row. Numeric
/// when every non-empty value is a number, otherwise case-insensitive text.
/// Empty values go last in both directions, and ties keep their order.
pub fn sort_order(values: &[String], descending: bool) -> Vec<usize> {
    let trimmed: Vec<&str> = values.iter().map(|v| v.trim()).collect();
    let numeric = trimmed.iter().any(|v| !v.is_empty())
        && trimmed
            .iter()
            .all(|v| v.is_empty() || v.parse::<f64>().is_ok());
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|&a, &b| {
        let (x, y) = (trimmed[a], trimmed[b]);
        match (x.is_empty(), y.is_empty()) {
            (true, true) => return std::cmp::Ordering::Equal,
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            (false, false) => {}
        }
        let ord = if numeric {
            let p = x.parse::<f64>().unwrap_or(0.0);
            let q = y.parse::<f64>().unwrap_or(0.0);
            p.partial_cmp(&q).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            x.to_lowercase().cmp(&y.to_lowercase())
        };
        if descending {
            ord.reverse()
        } else {
            ord
        }
    });
    order
}

/// Where a dragged row or column lands: the item is taken out, then put back
/// so that it occupies the slot `to` was in.
pub fn moved<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from < items.len() && to < items.len() && from != to {
        let item = items.remove(from);
        items.insert(to, item);
    }
}

/// What the right-click menu needs to know about the cell it opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuFacts {
    /// Index among the body rows; `None` in the header.
    pub body_row: Option<usize>,
    pub body_rows: usize,
    pub col: usize,
    pub cols: usize,
    pub align: Align,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: &'static str,
    pub action: &'static str,
    pub enabled: bool,
    pub ticked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuEntry {
    Sep,
    Item(MenuItem),
}

/// The table menu for this cell.
pub fn table_menu(f: &MenuFacts) -> Vec<MenuEntry> {
    let on_body = f.body_row.is_some();
    let item = |label, action, enabled| {
        MenuEntry::Item(MenuItem {
            label,
            action,
            enabled,
            ticked: false,
        })
    };
    let align = |label, action, a: Align| {
        MenuEntry::Item(MenuItem {
            label,
            action,
            enabled: true,
            ticked: f.align == a,
        })
    };
    vec![
        item("Insert row", "tbl_add_row", true),
        item("Insert column", "tbl_add_col", true),
        MenuEntry::Sep,
        item(
            "Move row up",
            "tbl_move_row_up",
            f.body_row.is_some_and(|r| r > 0),
        ),
        item(
            "Move row down",
            "tbl_move_row_down",
            f.body_row.is_some_and(|r| r + 1 < f.body_rows),
        ),
        item("Move column left", "tbl_move_col_left", f.col > 0),
        item(
            "Move column right",
            "tbl_move_col_right",
            f.col + 1 < f.cols,
        ),
        item("Duplicate row", "tbl_dup_row", on_body),
        item("Duplicate column", "tbl_dup_col", true),
        MenuEntry::Sep,
        align("Align default", "tbl_align_none", Align::None),
        align("Align left", "tbl_align_left", Align::Left),
        align("Align center", "tbl_align_center", Align::Center),
        align("Align right", "tbl_align_right", Align::Right),
        MenuEntry::Sep,
        item("Sort A to Z", "tbl_sort_asc", true),
        item("Sort Z to A", "tbl_sort_desc", true),
        item("Copy as Markdown", "tbl_copy_md", true),
        item("Duplicate table", "tbl_dup_table", true),
        MenuEntry::Sep,
        item("Delete row", "tbl_del_row", on_body),
        item("Delete column", "tbl_del_col", f.cols > 1),
        item("Delete table", "tbl_del_table", true),
    ]
}

/// May this table action run on this cell? The one place that decides, so the
/// right-click menu and the app menu can't disagree.
pub fn menu_allows(facts: &MenuFacts, action: &str) -> bool {
    table_menu(facts)
        .iter()
        .any(|e| matches!(e, MenuEntry::Item(i) if i.action == action && i.enabled))
}

/// The entries left showing for a filter: everything when it's empty, otherwise
/// the items containing it, ignoring case, with no separators.
pub fn menu_visible(entries: &[MenuEntry], filter: &str) -> Vec<MenuEntry> {
    if filter.is_empty() {
        return entries.to_vec();
    }
    let needle = filter.to_lowercase();
    entries
        .iter()
        .filter(|e| matches!(e, MenuEntry::Item(i) if i.label.to_lowercase().contains(&needle)))
        .copied()
        .collect()
}

/// The next enabled item from `from`, wrapping round. With nothing highlighted
/// yet, down starts at the top and up at the bottom.
pub fn step_menu(visible: &[MenuEntry], from: Option<usize>, down: bool) -> Option<usize> {
    let n = visible.len();
    let enabled = |i: usize| matches!(visible[i], MenuEntry::Item(it) if it.enabled);
    for k in 1..=n {
        let i = match (from, down) {
            (Some(f), true) => (f + k) % n,
            (Some(f), false) => (f + n - k % n) % n,
            (None, true) => k - 1,
            (None, false) => n - k,
        };
        if enabled(i) {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(body_row: Option<usize>) -> MenuFacts {
        MenuFacts {
            body_row,
            body_rows: 3,
            col: 1,
            cols: 3,
            align: Align::Center,
        }
    }

    fn enabled(entries: &[MenuEntry], action: &str) -> bool {
        entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Item(i) if i.action == action => Some(i.enabled),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn row_actions_are_off_on_the_header() {
        let m = table_menu(&facts(None));
        for a in [
            "tbl_move_row_up",
            "tbl_move_row_down",
            "tbl_dup_row",
            "tbl_del_row",
        ] {
            assert!(!enabled(&m, a), "{a}");
        }
        assert!(enabled(&m, "tbl_add_row"));
    }

    #[test]
    fn moves_stop_at_the_edges() {
        let first = table_menu(&facts(Some(0)));
        assert!(!enabled(&first, "tbl_move_row_up"));
        assert!(enabled(&first, "tbl_move_row_down"));
        let last = table_menu(&facts(Some(2)));
        assert!(enabled(&last, "tbl_move_row_up"));
        assert!(!enabled(&last, "tbl_move_row_down"));
        let left = table_menu(&MenuFacts {
            col: 0,
            ..facts(Some(1))
        });
        assert!(!enabled(&left, "tbl_move_col_left"));
        let right = table_menu(&MenuFacts {
            col: 2,
            ..facts(Some(1))
        });
        assert!(!enabled(&right, "tbl_move_col_right"));
    }

    #[test]
    fn the_only_column_cannot_be_deleted() {
        let one = table_menu(&MenuFacts {
            col: 0,
            cols: 1,
            ..facts(Some(0))
        });
        assert!(!enabled(&one, "tbl_del_col"));
        assert!(enabled(&one, "tbl_del_table"));
    }

    #[test]
    fn allows_follows_the_menu() {
        let header = facts(None);
        assert!(!menu_allows(&header, "tbl_del_row"));
        assert!(menu_allows(&header, "tbl_add_row"));
        assert!(menu_allows(&header, "tbl_add_col"));
        let one = MenuFacts {
            col: 0,
            cols: 1,
            ..facts(Some(0))
        };
        assert!(!menu_allows(&one, "tbl_del_col"));
        assert!(menu_allows(&one, "tbl_del_table"));
        assert!(!menu_allows(&one, "tbl_no_such_action"));
    }

    #[test]
    fn only_the_current_alignment_is_ticked() {
        let ticked: Vec<&str> = table_menu(&facts(Some(0)))
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item(i) if i.ticked => Some(i.action),
                _ => None,
            })
            .collect();
        assert_eq!(ticked, vec!["tbl_align_center"]);
    }

    #[test]
    fn filtering_ignores_case_and_drops_separators() {
        let m = table_menu(&facts(Some(1)));
        assert_eq!(menu_visible(&m, ""), m);
        let v = menu_visible(&m, "SORT");
        assert_eq!(v.len(), 2);
        assert!(v.iter().all(|e| matches!(e, MenuEntry::Item(_))));
        assert!(menu_visible(&m, "zzz").is_empty());
    }

    #[test]
    fn stepping_skips_disabled_items_and_separators() {
        let i = |enabled| {
            MenuEntry::Item(MenuItem {
                label: "x",
                action: "x",
                enabled,
                ticked: false,
            })
        };
        let v = [i(false), MenuEntry::Sep, i(true), i(false), i(true)];
        assert_eq!(step_menu(&v, None, true), Some(2));
        assert_eq!(step_menu(&v, None, false), Some(4));
        assert_eq!(step_menu(&v, Some(2), true), Some(4));
        assert_eq!(step_menu(&v, Some(4), true), Some(2));
        assert_eq!(step_menu(&v, Some(2), false), Some(4));
        assert_eq!(step_menu(&v, Some(4), false), Some(2));
        assert_eq!(step_menu(&[i(false)], None, true), None);
        assert_eq!(step_menu(&[], None, true), None);
    }

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pads_columns_to_the_widest_cell() {
        let md = format_table(
            &[row(&["Name", "N"]), row(&["a", "100"])],
            &[Align::None, Align::None],
            &[],
        );
        assert_eq!(md, "| Name | N   |\n| ---- | --- |\n| a    | 100 |");
    }

    #[test]
    fn alignment_shows_in_the_padding_and_the_delimiter() {
        let md = format_table(
            &[row(&["left", "mid", "right"]), row(&["a", "b", "c"])],
            &[Align::Left, Align::Center, Align::Right],
            &[],
        );
        assert_eq!(
            md,
            "| left | mid | right |\n| :--- | :-: | ----: |\n| a    |  b  |     c |"
        );
    }

    #[test]
    fn wide_characters_count_double() {
        let md = format_table(&[row(&["日本語"]), row(&["abc"])], &[], &[]);
        assert_eq!(md, "| 日本語 |\n| ------ |\n| abc    |");
    }

    #[test]
    fn short_rows_are_filled_out() {
        let md = format_table(&[row(&["a", "b"]), row(&["c"])], &[], &[]);
        assert_eq!(md, "| a   | b   |\n| --- | --- |\n| c   |     |");
    }

    #[test]
    fn widths_go_on_the_line_after_the_table() {
        let md = format_table(&[row(&["a", "b"])], &[], &[Some(120), None]);
        assert!(md.ends_with("| --- | --- |\n<!-- zen:cols=120,auto -->"));
    }

    #[test]
    fn an_unresized_table_has_no_comment() {
        assert_eq!(cols_comment(&[]), None);
        assert_eq!(cols_comment(&[None, None]), None);
    }

    #[test]
    fn the_width_comment_round_trips() {
        let widths = parse_cols_comment("<!-- zen:cols=120,auto,90 -->\n").unwrap();
        assert_eq!(widths, vec![Some(120), None, Some(90)]);
        assert_eq!(
            cols_comment(&widths).as_deref(),
            Some("<!-- zen:cols=120,auto,90 -->")
        );
    }

    #[test]
    fn other_comments_are_not_widths() {
        assert_eq!(parse_cols_comment("<!-- a note -->"), None);
        assert_eq!(parse_cols_comment("zen:cols=1,2"), None);
    }

    #[test]
    fn a_bad_width_falls_back_to_auto() {
        assert_eq!(
            parse_cols_comment("<!-- zen:cols=wide,0,12 -->"),
            Some(vec![None, None, Some(12)])
        );
    }

    #[test]
    fn numbers_sort_as_numbers() {
        let v = row(&["10", "9", "100"]);
        assert_eq!(sort_order(&v, false), vec![1, 0, 2]);
        assert_eq!(sort_order(&v, true), vec![2, 0, 1]);
    }

    #[test]
    fn text_sorts_ignoring_case() {
        assert_eq!(sort_order(&row(&["b", "A", "c"]), false), vec![1, 0, 2]);
    }

    #[test]
    fn one_word_makes_the_whole_column_text() {
        assert_eq!(sort_order(&row(&["10", "9", "x"]), false), vec![0, 1, 2]);
    }

    #[test]
    fn empty_cells_sort_last_in_both_directions() {
        let v = row(&["2", "", "1"]);
        assert_eq!(sort_order(&v, false), vec![2, 0, 1]);
        assert_eq!(sort_order(&v, true), vec![0, 2, 1]);
    }

    #[test]
    fn a_tie_keeps_its_order() {
        assert_eq!(sort_order(&row(&["a", "A", "a"]), false), vec![0, 1, 2]);
    }

    #[test]
    fn a_dragged_item_takes_the_slot_it_was_dropped_on() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        moved(&mut v, 0, 2);
        assert_eq!(v, vec!['b', 'c', 'a', 'd']);
        moved(&mut v, 3, 0);
        assert_eq!(v, vec!['d', 'b', 'c', 'a']);
        moved(&mut v, 1, 9);
        assert_eq!(v, vec!['d', 'b', 'c', 'a']);
    }
}
