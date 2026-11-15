//! mdedit v2: the GPUI app. Work in progress on the `v2-gpui` branch; see
//! `docs/v2-notes.md`.

use gpui_kit::component::TitleBar;
use gpui_kit::*;

struct Welcome;

impl Render for Welcome {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(TitleBar::new().child("mdedit"))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .child("mdedit 2 — under construction"),
            )
    }
}

fn main() {
    mdedit_core::detach::maybe_detach();
    application().run(|cx| {
        init(cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1100.), px(760.)),
                cx,
            ))),
            window_min_size: Some(size(px(680.), px(440.))),
            ..TitleBar::window_options()
        };
        open_window(options, cx, |_, cx| cx.new(|_| Welcome)).expect("open the window");
    });
}
