//! `cargo run -p mdedit-pdf --example typeset -- in.md out.pdf`: typeset a
//! markdown file the way Export as PDF does, for checking the output.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = (&args[1], &args[2]);
    let md = std::fs::read_to_string(input).expect("read the markdown");
    let fonts: Vec<&'static [u8]> = vec![
        include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
        include_bytes!("../../../assets/fonts/Inter-Medium.ttf"),
        include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
        include_bytes!("../../../assets/fonts/Inter-Bold.ttf"),
        include_bytes!("../../../assets/fonts/Inter-Italic.ttf"),
        include_bytes!("../../../assets/fonts/Inter-BoldItalic.ttf"),
        include_bytes!("../../../assets/fonts/SourceSerif4-Regular.ttf"),
        include_bytes!("../../../assets/fonts/SourceSerif4-Bold.ttf"),
        include_bytes!("../../../assets/fonts/SourceSerif4-It.ttf"),
        include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
        include_bytes!("../../../assets/fonts/JetBrainsMono-Bold.ttf"),
    ];
    let mut db = mdedit_core::mermaid::fontdb::Database::new();
    for f in &fonts {
        db.load_font_data(f.to_vec());
    }
    let p = mdedit_core::themes::system_palette(false);
    let c = [
        p.accent,
        p.alert_tip,
        p.alert_warning,
        p.alert_important,
        p.alert_caution,
        p.alert_note,
        p.text_muted,
        p.diagram_border,
    ];
    let opts = mdedit_pdf::Options {
        style: mdedit_core::prefs::Style::GitHub,
        numbered: false,
        letter: false,
        fonts,
        base_dir: std::path::Path::new(input)
            .parent()
            .map(|p| p.to_path_buf()),
        diagrams: Some(mdedit_core::mermaid::DrawOptions {
            colors: mdedit_core::mermaid::Colors {
                bg: p.bg,
                fg: p.text,
                muted: p.text_muted,
                line: p.text,
                surface: p.bg_soft,
                surface_alt: p.diagram_surface_alt,
                border: p.diagram_border,
                accent: p.accent,
                critical: p.danger,
                series: c,
            },
            font_family: "Inter".into(),
            font_family_mono: "JetBrains Mono".into(),
            fontdb: std::sync::Arc::new(db),
        }),
    };
    let t = std::time::Instant::now();
    let pdf = mdedit_pdf::markdown_to_pdf(&md, input, &opts).expect("typeset");
    std::fs::write(output, &pdf).expect("write the PDF");
    eprintln!("{} bytes in {:?}", pdf.len(), t.elapsed());
}
