# mdedit v2 — engineering notes

Findings from the M0 spike (2026-11-16, gpui-kit 0.9.1 / gpui-pre 0.3.12,
Arch, Wayland, RADV Vulkan). Kept here so later milestones don't relearn them.

## GPUI / gpui-kit

- `use gpui_kit::*;` is GPUI itself. Start with `gpui_kit::application()`,
  call `gpui_kit::init(cx)` before any window, and open windows with
  `gpui_kit::open_window(options, cx, |window, cx| view)`, which wraps the
  view in a `Root` (dialogs, sheets, notifications).
- Client-side decorations: `TitleBar::new().child(...)` with
  `WindowOptions { ..TitleBar::window_options() }`. On Linux it draws its own
  minimise/maximise/close buttons, and `on_close_window` overrides close.
- **Fonts must be explicit.** The default UI family here is "Adwaita Sans",
  which has no bold face installed, so `FontWeight::BOLD` renders regular.
  Bundle the fonts (Inter, Source Serif 4, JetBrains Mono) and name them.
- The custom editor element follows `gpui-pre/examples/input.rs`: an
  `Element` whose `prepaint` shapes text through
  `window.text_system().shape_text(text, font_size, &runs, Some(wrap), None)`,
  giving one `WrappedLine` per hard line. Its `paint` calls
  `window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx)`.
  The entity implements `EntityInputHandler` (typing and IME).
  - `shape_text` takes one font size per call, so shape each block at its
    own size. `TextRun` carries font (family, weight, style), colour,
    background, underline and strikethrough.
  - `WrappedLine::position_for_index` and `closest_index_for_position`
    handle caret placement and hit-testing. `line.size(line_height)` gives
    the wrapped height.
  - Measured: about 3 ms for the first layout and about 60 µs per frame after
    that, on a debug build of the toy document.
- Images: `img(Arc::new(Image::from_bytes(ImageFormat::Svg, bytes)))`.
- Clipboard: `ClipboardEntry` is String, Image or ExternalPaths only, and
  there is no text/html. Smart paste needs `arboard` or a platform read (M4).
- Tests: add `gpui-kit = { features = ["test-support"] }` to
  dev-dependencies and use `#[gpui_kit::test] fn t(cx: &mut TestAppContext)`
  with `cx.add_window_view`, `simulate_keystrokes("left down")` and
  `simulate_input("é")`. These are headless and pass here.
  - **Don't glob-import `gpui_kit::*` in a test module.** It brings GPUI's
    `test` macro in over Rust's `#[test]` (recursion-limit error), so import
    names explicitly.
  - Adding `test-support` to an *existing* lockfile can fail on a `taffy`
    conflict, through the optional `gpui-fast` dependency that weak features
    drag into resolution. Regenerating the lockfile resolves it.

## Mermaid: bezel-mermaid 0.2.26 (lib name `mermaid`)

- `mermaid::render(src, &Options, &impl Host) -> Result<Drawing{svg,width,height,..}, Error>`.
  `mermaid::Metrics` is the Arial width-table host. Implement `Host` over
  GPUI's text system for exact label widths.
- It is a port of **Pele 1.6.0**, while v1 vendored Pele 1.8.0.
- Set `Options::colors = Some(Colors{bg,fg,muted,line,surface,surface_alt,border,accent,critical,series:[_;8]})`
  (each `[u8;4]`), and the SVG gets concrete colours in place of every
  `var(--bezel-*)`. No post-processing is needed for resvg.
- **Text must be flattened before GPUI draws it.** usvg ignores the root
  `style="font-family:…"` and falls back to `Options::font_family`, which
  defaults to "Times New Roman". That isn't installed here, so a symbol font
  matches and labels come out as Greek. Parse with our own
  `usvg::Options { fontdb, font_family: <app sans>, .. }` and write it back
  with `tree.to_string(&WriteOptions::default())`, which turns text into
  paths. Build the fontdb once, from the bundled fonts, and cache it.
  - Debug-build cost: about 2 ms to render plus about 60 ms to flatten,
    mostly `load_system_fonts` on every call.
- The flattened SVG is also what the Typst PDF embeds, so diagrams look the
  same everywhere.

## Math: latex-rust 3.0.3

- `latex_rust::latex_to_svg(src, &MathFont::stix_two_math()?, &SvgOptions{display, ..SvgOptions::new()})`.
  It outputs glyphs as paths, needs no fonts, takes about 3 ms in debug, and
  draws correctly through `img()`.
- `render_svg_with_metrics` returns `SvgImage{width_pt,height_pt,baseline_pt}`
  for aligning inline math to the text baseline.
- HTML export keeps math-core's MathML.

## M2 findings: the shell (gpui-kit 0.9.1)

- **Dock isn't used for document tabs.** It can't veto a close (no
  unsaved-changes prompt), can't close its last panel, has no tab context
  menu, and can't put tabs in the title bar. `workspace.rs` draws its own
  panes and tabs: `h_resizable` for splits, and GPUI
  `on_drag`/`drag_over`/`on_drop` for moving tabs.
- **Keys:** write `secondary-…` and build bindings with `KeyBinding::load`,
  because `KeyBinding::new("secondary-…")` panics. Actions carrying data
  need `#[derive(gpui_kit::Action)] #[action(namespace = mdedit, no_json)]`.
- **Menus:** on Linux, `cx.set_menus` draws nothing. The ☰ button is
  `Button::dropdown_menu(|menu, window, cx| …)`, which is rebuilt every time
  it opens. Shortcut hints show automatically. `ContextMenuExt::context_menu`
  works on any element that has an id.
- **Dialogs:**
  - `window.open_dialog(cx, |dialog, …| …)` needs gpui-kit's `Root`, which
    `gpui_kit::open_window` adds.
  - `on_cancel` fires on Escape and on a backdrop click, never on
    `close_dialog`. So a button closes the dialog first and answers second.
  - A plain `Dialog` has no buttons until you give it a `DialogFooter`.
- **Closing:**
  - `window.on_window_should_close` catches the window manager closing the
    window.
  - TitleBar's own ✕ calls `remove_window`, which skips that check, so
    `TitleBar::on_close_window` dispatches `Quit` instead.
  - On Linux, `cx.activate` does nothing; use `window.activate_window()`,
    which needs an `app_id` on Wayland.
- **Theme:** start from `ThemeConfig::default()` with only the palette's
  colours set; the kit derives hover, list and tab shades from them.
  - **Setting `config.highlight` replaces the syntax palette.** Start it from
    `HighlightTheme::default_{dark,light}().style` and change only the
    editor colours.
- **File dialogs:** `cx.prompt_for_paths` and `prompt_for_new_path` take no
  type filters. Await them inside
  `cx.spawn_in(window, async move |this, cx| …)`, then call
  `this.update_in(…)`.
- **Linux file events:** `Application::on_open_urls` fires only on macOS.
  On Linux, files from later launches come through `single.rs`'s socket.
- **Text editing:** `EditorState` (with the `syntect` feature) handles
  markdown and source until M3. `set_value` emits no `Change` event;
  `insert` does.
- **Tests:**
  - Open a real rooted window with `gpui_kit::open_window` inside
    `cx.update`.
  - Drive it with `gpui_kit::test::TestWindowExt`: `press("ctrl-n")`,
    `click("dont-save")`, `render_frame`.
  - `AppState::init_ephemeral` keeps tests away from the real settings
    file.
