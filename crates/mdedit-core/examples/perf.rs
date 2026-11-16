//! How long the per-keystroke work takes on a large document:
//! `cargo run --release -p mdedit-core --example perf [lines]`.

use std::time::Instant;

use mdedit_core::{display, syntax};

const CHUNK: &str = "## Section heading\n\nA paragraph with **bold**, *italic*, `code` and a [link](https://example.com) that runs\nonto a second source line, as hard-wrapped text does.\n\n- a bullet item\n- another, with `code`\n  - nested\n1. first\n2. second\n- [ ] a task\n- [x] done\n\n> A quote with *emphasis*.\n\n| Name | Value |\n|------|------:|\n| a    | 1     |\n| b    | 2     |\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n";

fn main() {
    let lines: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let per = CHUNK.lines().count();
    let mut text = String::from("# A large document\n\n");
    while text.lines().count() < lines {
        text.push_str(CHUNK);
    }
    println!(
        "{} lines, {} KB ({} lines per chunk)",
        text.lines().count(),
        text.len() / 1024,
        per
    );

    let mid = text.len() / 2;
    let mid = (0..=mid).rev().find(|p| text.is_char_boundary(*p)).unwrap();
    let runs = 20;
    let (mut parse, mut rows, mut count, mut numbers) = (0f64, 0f64, 0f64, 0f64);
    for i in 0..runs {
        text.insert(mid + i, 'x');
        let t = Instant::now();
        let s = syntax::parse(&text);
        parse += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let r = display::rows(&text, &s, true, Some(s.line_at(mid)));
        rows += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let w = text.split_whitespace().count() + text.chars().count();
        count += t.elapsed().as_secs_f64();
        let t = Instant::now();
        let n = s.heading_numbers();
        numbers += t.elapsed().as_secs_f64();
        std::hint::black_box((r.len(), w, n.len()));
    }
    let ms = |s: f64| s * 1000. / runs as f64;
    println!(
        "per keystroke: parse {:.2} ms, rows {:.2} ms, counts {:.2} ms, heading numbers {:.2} ms",
        ms(parse),
        ms(rows),
        ms(count),
        ms(numbers)
    );
}
