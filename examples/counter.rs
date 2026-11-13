//! A counter with a name field: the smallest app fastgpui serves, and the app its tests drive.
//!
//! `cargo run --example counter` opens it on screen. With `GDP_SOCKET` set it serves the debug protocol instead.

use gpui_kit::component::{button::Button, input::{Input, InputState}};
use gpui_kit::{App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Window, WindowOptions, div, px};

struct Counter { count: usize, name: Entity<InputState> }

impl Render for Counter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().flex().flex_col().gap_4().p_4()
            .child(Input::new(&self.name).id("name").w(px(240.)))
            // The count changes in a spawned task, as work that waits on I/O would, so it shows only after the task runs.
            .child(Button::new("increment").label(format!("Count: {}", self.count)).on_click(cx.listener(|_, _, _, cx| {
                cx.spawn(async move |this, cx| this.update(cx, |this, cx| { this.count += 1; cx.notify() })).detach();
            })))
    }
}

fn start(cx: &mut App) {
    gpui_kit::init(cx);
    gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
        let name = cx.new(|cx| InputState::new(window, cx).default_value("hello world"));
        cx.new(|_| Counter { count: 0, name })
    }).expect("open the counter's window");
}

fn main() {
    if fastgpui::requested() { return fastgpui::serve(start) }
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(start);
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    /// The counter, served in process: commands describe themselves, a click increments the count, and a drag through single pointer
    /// steps, from inside the focused name field to past its start, selects its text for Ctrl+C to copy.
    #[test]
    fn a_session_drives_the_counter() {
        let mut s = fastgpui::Server::new(super::start).command("hello", "Greets `name`", &["name"], |_, _, r| Ok(json!(format!("hi {}", r["name"])))).start();
        let mut call = |req: Value| s.call(&req).unwrap();
        let names: Vec<Value> = call(json!({"cmd": "commands"})).as_array().unwrap().iter().map(|c| c["name"].clone()).collect();
        assert!(names.contains(&json!("click")) && names.contains(&json!("hello")));
        assert_eq!(call(json!({"cmd": "hello", "name": "ada"})), json!("hi \"ada\""));

        call(json!({"cmd": "click", "id": "increment"}));
        assert_eq!(call(json!({"cmd": "find", "id": "increment"}))["label"], "Count: 1");

        call(json!({"cmd": "click", "id": "name"}));
        let b = call(json!({"cmd": "find", "id": "name"}))["bounds"].clone();
        let (x, y, w, h) = (b[0].as_f64().unwrap(), b[1].as_f64().unwrap(), b[2].as_f64().unwrap(), b[3].as_f64().unwrap());
        let (from, to, mid) = (x + w - 16., x + 2., y + h / 2.);
        call(json!({"cmd": "pointer_move", "x": from, "y": mid}));
        call(json!({"cmd": "pointer_down", "x": from, "y": mid}));
        for i in 1..=8 { call(json!({"cmd": "pointer_move", "x": from + (to - from) * i as f64 / 8., "y": mid, "button": "left"})); }
        call(json!({"cmd": "pointer_up", "x": to, "y": mid}));
        call(json!({"cmd": "press", "key": "ctrl-c"}));
        assert_eq!(call(json!({"cmd": "clipboard"})), json!("hello world"));
        assert!(s.call(&json!({"cmd": "nope"})).is_err());
    }
}
