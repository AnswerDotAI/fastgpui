//! Serves a GPUI app off screen so that a script can drive it, as Chrome's debugging port lets fastcdp drive a browser.
//!
//! An app opts in through a cargo feature, by convention `gdp`, that turns on its `fastgpui` dependency. Its `main` calls `serve` when
//! `requested` finds `GDP_SOCKET` set. `serve` runs the app's start-up function in GPUI's headless app, with the real text system and
//! renderer, and answers JSON requests, one per line, on the Unix socket `GDP_SOCKET` names. Each request is an object with a `cmd` and
//! its fields; the reply is `{"ok": true, "result": ...}` or `{"ok": false, "error": ...}`. The `commands` request describes every command.
//! Commands are gpui-kit's test operations, run in their own window update. After each command the server runs every ready task and
//! draws a frame. It never waits for work that isn't ready, such as a network reply, so clients poll for results.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gpui_kit::base::test_support as seen;
use gpui_kit::platform::{current_headless_renderer, current_platform};
use gpui_kit::test::{ClickOptions, ElementSnapshot, ScopedWindow, TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AnyWindowHandle, App, ElementId, HeadlessAppContext, Modifiers, MouseButton, Pixels, Point, ScrollDelta, Window, point, px};
use serde_json::{Value, json};
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::{panic::AssertUnwindSafe, path::Path, sync::Arc, time::Duration};

/// The environment variable that names the socket to serve on.
pub const SOCKET: &str = "GDP_SOCKET";

/// Whether this run should serve the debug protocol instead of opening the app on screen.
pub fn requested() -> bool { std::env::var_os(SOCKET).is_some() }

/// Serves the app that `start` opens, with fastgpui's commands, on the socket `GDP_SOCKET` names.
pub fn serve(start: impl FnOnce(&mut App) + 'static) { Server::new(start).serve() }

type Run = Box<dyn Fn(&mut Window, &mut App, &Value) -> Result<Value, String>>;

struct Command { name: &'static str, doc: &'static str, params: &'static [&'static str], run: Run }

/// The app's start-up function and the commands it serves: fastgpui's own, then any the app adds with `command`.
pub struct Server { start: Box<dyn FnOnce(&mut App)>, commands: Vec<Command> }

impl Server {
    pub fn new(start: impl FnOnce(&mut App) + 'static) -> Self { Self { start: Box::new(start), commands: builtins() } }

    /// Adds command `name`, which runs `run` in the target window with the request's fields. `params` names the fields it reads.
    pub fn command(
        mut self,
        name: &'static str,
        doc: &'static str,
        params: &'static [&'static str],
        run: impl Fn(&mut Window, &mut App, &Value) -> Result<Value, String> + 'static,
    ) -> Self {
        self.commands.push(cmd(name, doc, params, run));
        self
    }

    /// Opens the app off screen, ready for `Session::call`.
    pub fn start(self) -> Session {
        let mut cx = HeadlessAppContext::with_platform(current_platform(true).text_system(), Arc::new(gpui_kit::assets::AllAssets), current_headless_renderer);
        // Apps do real I/O, such as network requests on their own threads; parking lets GPUI's test scheduler accept it.
        cx.allow_parking();
        cx.update(self.start);
        settle(&mut cx);
        Session { cx, commands: self.commands }
    }

    /// Opens the app off screen and serves it on the socket `GDP_SOCKET` names, until the socket fails.
    pub fn serve(self) {
        let socket = std::env::var_os(SOCKET).expect("GDP_SOCKET names the socket to serve on");
        if let Err(e) = self.start().serve_at(Path::new(&socket)) { panic!("fastgpui: serving {socket:?} failed: {e}") }
    }
}

/// An app running off screen, and the commands it answers.
pub struct Session { cx: HeadlessAppContext, commands: Vec<Command> }

const OWN: &[(&str, &str, &[&str])] = &[
    ("commands", "Every command, with its doc and the request fields it reads", &[]),
    ("windows", "Each window's id and size", &[]),
    ("screenshot", "The window as rendered off screen: `width`, `height` and base64 RGBA pixels in `rgba`", &["window"]),
    ("advance", "Moves GPUI's simulated clock forward by `ms`, so timers due by then fire", &["ms"]),
];

impl Session {
    /// Runs request `req` and settles. A command acts on the window whose id is `req["window"]`, or else the first window.
    pub fn call(&mut self, req: &Value) -> Result<Value, String> {
        let Self { cx, commands } = self;
        let name = req["cmd"].as_str().unwrap_or_default();
        let reply = match name {
            "commands" => Ok(OWN.iter().map(|(n, d, p)| json!({"name": n, "doc": d, "params": p}))
                .chain(commands.iter().map(|c| json!({"name": c.name, "doc": c.doc, "params": c.params}))).collect()),
            "windows" => Ok(cx.update(|cx| cx.windows()).into_iter().filter_map(|w| cx.update_window(w, |_, window, _| {
                let size = window.viewport_size();
                json!({"window": w.window_id().as_u64(), "size": [f32::from(size.width), f32::from(size.height)]})
            }).ok()).collect()),
            "screenshot" => {
                let w = target(cx, req)?;
                let img = cx.capture_screenshot(w).map_err(|e| e.to_string())?;
                Ok(json!({"width": img.width(), "height": img.height(), "rgba": STANDARD.encode(img.as_raw())}))
            }
            "advance" => { cx.advance_clock(Duration::from_millis(req["ms"].as_u64().unwrap_or_default())); Ok(Value::Null) }
            _ => {
                let c = commands.iter().find(|c| c.name == name).ok_or(format!("unknown command {name:?}"))?;
                let w = target(cx, req)?;
                cx.update_window(w, |_, window, cx| { window.render_frame(cx); (c.run)(window, cx, req) }).map_err(|e| e.to_string())?
            }
        };
        settle(cx);
        reply
    }

    fn serve_at(mut self, socket: &Path) -> std::io::Result<()> {
        let _ = std::fs::remove_file(socket);
        for stream in UnixListener::bind(socket)?.incoming() {
            let stream = stream?;
            for line in BufReader::new(&stream).lines() {
                let reply = match serde_json::from_str::<Value>(&line?) {
                    Ok(req) => std::panic::catch_unwind(AssertUnwindSafe(|| self.call(&req))).unwrap_or_else(|e| Err(panic_text(e))),
                    Err(e) => Err(format!("bad request: {e}")),
                };
                let reply = match reply { Ok(v) => json!({"ok": true, "result": v}), Err(e) => json!({"ok": false, "error": e}) };
                writeln!(&stream, "{reply}")?;
            }
        }
        Ok(())
    }
}

fn panic_text(e: Box<dyn std::any::Any + Send>) -> String {
    e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panicked".into())
}

/// Runs every ready task, then draws every window.
fn settle(cx: &mut HeadlessAppContext) {
    cx.run_until_parked();
    for w in cx.update(|cx| cx.windows()) { cx.update_window(w, |_, window, cx| window.render_frame(cx)).ok(); }
}

fn target(cx: &mut HeadlessAppContext, req: &Value) -> Result<AnyWindowHandle, String> {
    let all = cx.update(|cx| cx.windows());
    match req["window"].as_u64() {
        Some(id) => all.into_iter().find(|w| w.window_id().as_u64() == id).ok_or(format!("no window {id}")),
        None => all.into_iter().next().ok_or("no window".into()),
    }
}

fn text(req: &Value, key: &str) -> Result<String, String> { req[key].as_str().map(str::to_string).ok_or(format!("needs `{key}`")) }
fn id(req: &Value, key: &str) -> Result<ElementId, String> { text(req, key).map(ElementId::from) }
fn num(req: &Value, key: &str) -> f32 { req[key].as_f64().unwrap_or_default() as f32 }
fn at(req: &Value) -> Point<Pixels> { point(px(num(req, "x")), px(num(req, "y"))) }

/// `req["within"]`: one element id, or a path of them, naming nested GPUI identity scopes.
fn within(req: &Value) -> Vec<ElementId> {
    match &req["within"] {
        Value::String(s) => vec![ElementId::from(s.clone())],
        Value::Array(ids) => ids.iter().filter_map(Value::as_str).map(|s| ElementId::from(s.to_string())).collect(),
        _ => vec![],
    }
}

fn button(req: &Value) -> MouseButton {
    match req["button"].as_str() { Some("right") => MouseButton::Right, Some("middle") => MouseButton::Middle, _ => MouseButton::Left }
}

fn modifiers(req: &Value) -> Modifiers {
    let has = |m: &str| req["modifiers"].as_array().into_iter().flatten().any(|n| n == m);
    Modifiers { control: has("ctrl"), alt: has("alt"), shift: has("shift"), platform: has("cmd"), function: has("fn") }
}

fn element(e: &ElementSnapshot) -> Value {
    let b = e.bounds();
    json!({
        "path": e.path().iter().map(ElementId::to_string).collect::<Vec<_>>(), "role": e.role().map(|r| format!("{r:?}")),
        "label": e.label(), "value": e.value(), "description": e.description(), "placeholder": e.placeholder(), "visible": e.visible(),
        "focused": e.focused(), "checked": e.checked(), "selected": e.selected(), "expanded": e.expanded(), "disabled": e.disabled(),
        "bounds": [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)],
    })
}

fn all(es: Vec<ElementSnapshot>) -> Value { es.iter().map(element).collect() }

fn nest<R>(s: &mut ScopedWindow, rest: &[ElementId], f: impl FnOnce(&mut ScopedWindow) -> R) -> R {
    match rest.split_first() { None => f(s), Some((id, rest)) => nest(&mut s.within(id.clone()), rest, f) }
}

/// Runs `$body` with `$w` bound to `$window`, or to the scope that the request's `within` names.
macro_rules! scoped {
    ($window:expr, $req:expr, |$w:ident| $body:expr) => {{
        let path = within($req);
        match path.split_first() {
            None => { let $w = $window; $body }
            Some((first, rest)) => nest(&mut $window.within(first.clone()), rest, |$w| $body),
        }
    }};
}

fn cmd(
    name: &'static str,
    doc: &'static str,
    params: &'static [&'static str],
    run: impl Fn(&mut Window, &mut App, &Value) -> Result<Value, String> + 'static,
) -> Command {
    Command { name, doc, params, run: Box::new(run) }
}

fn builtins() -> Vec<Command> {
    vec![
        cmd("elements", "Every element the last frame registered, by position", &["within"], |w, _, r| scoped!(w, r, |s| Ok(all(s.elements())))),
        cmd("find", "Element `id`; an error if it's missing or ambiguous", &["id", "within"], |w, _, r| scoped!(w, r, |s| Ok(element(&s.find(id(r, "id")?))))),
        cmd("try_find", "Element `id`, or null if it's missing", &["id", "within"],
            |w, _, r| scoped!(w, r, |s| Ok(s.try_find(id(r, "id")?).as_ref().map_or(Value::Null, element)))),
        cmd("find_all", "Every element with id `id`, including invisible ones", &["id", "within"], |w, _, r| scoped!(w, r, |s| Ok(all(s.find_all(id(r, "id")?))))),
        cmd("find_by_label", "The element whose accessible label is `label`", &["label", "within"],
            |w, _, r| scoped!(w, r, |s| Ok(element(&s.find_by_label(&text(r, "label")?))))),
        cmd("find_all_by_label", "Every element whose accessible label is `label`", &["label", "within"],
            |w, _, r| scoped!(w, r, |s| Ok(all(s.find_all_by_label(&text(r, "label")?))))),
        cmd("find_all_by_role", "Every element with AccessKit role `role`, such as `Button`", &["role", "within"], |w, _, r| {
            let role = text(r, "role")?;
            scoped!(w, r, |s| Ok(all(s.elements().into_iter().filter(|e| e.role().is_some_and(|x| format!("{x:?}") == role)).collect())))
        }),
        cmd("texts", "Visible text that gpui-kit components register, with its element path", &["within"], |w, _, r| {
            let scope = within(r).iter().fold(vec![], |scope, id| seen::scope(w, &scope, id));
            Ok(seen::texts(w).iter().filter(|t| t.visible() && t.path().starts_with(&scope))
                .map(|t| json!({"path": t.path().iter().map(ElementId::to_string).collect::<Vec<_>>(), "text": t.text()})).collect())
        }),
        cmd("click", "A left click at the centre of element `id`", &["id", "within"], |w, cx, r| scoped!(w, r, |s| { s.click(id(r, "id")?, cx); Ok(Value::Null) })),
        cmd("click_at", "A left click at offset (`x`, `y`) from element `id`'s top-left corner", &["id", "x", "y", "within"],
            |w, cx, r| scoped!(w, r, |s| { s.click_at(id(r, "id")?, at(r), cx); Ok(Value::Null) })),
        cmd("click_with_options", "`count` clicks of `button` on element `id`, holding `modifiers`, at offset (`x`, `y`) if given",
            &["id", "count", "button", "modifiers", "x", "y", "within"], |w, cx, r| {
                let mut o = ClickOptions::new().with_count(r["count"].as_u64().unwrap_or(1) as usize).with_button(button(r)).with_modifiers(modifiers(r));
                if !r["x"].is_null() { o = o.with_offset(at(r)) }
                scoped!(w, r, |s| { s.click_with_options(id(r, "id")?, o, cx); Ok(Value::Null) })
            }),
        cmd("right_click", "A right click at the centre of element `id`", &["id", "within"], |w, cx, r| scoped!(w, r, |s| { s.right_click(id(r, "id")?, cx); Ok(Value::Null) })),
        cmd("double_click", "A double click at the centre of element `id`", &["id", "within"], |w, cx, r| scoped!(w, r, |s| { s.double_click(id(r, "id")?, cx); Ok(Value::Null) })),
        cmd("hover", "Moves the pointer to the centre of element `id`", &["id", "within"], |w, cx, r| scoped!(w, r, |s| { s.hover(id(r, "id")?, cx); Ok(Value::Null) })),
        cmd("scroll", "Turns the wheel over element `id` by (`dx`, `dy`) pixels; negative `dy` scrolls down", &["id", "dx", "dy", "within"], |w, cx, r| {
            let delta = ScrollDelta::Pixels(point(px(num(r, "dx")), px(num(r, "dy"))));
            scoped!(w, r, |s| { s.scroll(id(r, "id")?, delta, cx); Ok(Value::Null) })
        }),
        cmd("press", "Presses `key`, in GPUI's syntax such as `enter` or `ctrl-shift-n`, at the focus", &["key", "within"],
            |w, cx, r| scoped!(w, r, |s| { s.press(&text(r, "key")?, cx); Ok(Value::Null) })),
        cmd("input", "Types `text` at the focus, one character at a time", &["text", "within"], |w, cx, r| scoped!(w, r, |s| { s.input(&text(r, "text")?, cx); Ok(Value::Null) })),
        cmd("pointer_down", "Presses `button` at window position (`x`, `y`)", &["x", "y", "button"], |w, cx, r| { w.pointer_down(at(r), button(r), cx); Ok(Value::Null) }),
        cmd("pointer_move", "Moves the pointer to (`x`, `y`), with `button` held if given", &["x", "y", "button"],
            |w, cx, r| { w.pointer_move(at(r), (!r["button"].is_null()).then(|| button(r)), cx); Ok(Value::Null) }),
        cmd("pointer_up", "Releases `button` at window position (`x`, `y`)", &["x", "y", "button"], |w, cx, r| { w.pointer_up(at(r), button(r), cx); Ok(Value::Null) }),
        cmd("key_down", "Sends key-down for `key`; `held` marks an auto-repeat", &["key", "held"],
            |w, cx, r| { w.key_down(&text(r, "key")?, r["held"].as_bool().unwrap_or_default(), cx); Ok(Value::Null) }),
        cmd("key_up", "Sends key-up for `key`", &["key"], |w, cx, r| { w.key_up(&text(r, "key")?, cx); Ok(Value::Null) }),
        cmd("change_modifiers", "Holds exactly `modifiers`, a list such as `[\"ctrl\", \"shift\"]`", &["modifiers"],
            |w, cx, r| { w.change_modifiers(modifiers(r), cx); Ok(Value::Null) }),
        cmd("clipboard", "The clipboard's text", &[], |_, cx, _| Ok(json!(cx.read_from_clipboard().and_then(|c| c.text())))),
        cmd("actions", "The names of the actions available at the focus", &[], |w, cx, _| {
            let mut names: Vec<_> = w.available_actions(cx).iter().map(|a| a.name()).collect();
            names.sort();
            names.dedup();
            Ok(json!(names))
        }),
        cmd("action", "Dispatches action `name`, such as `editor::Copy`, with JSON `data` if the action takes any", &["name", "data"], |w, cx, r| {
            let action = cx.build_action(&text(r, "name")?, r.get("data").filter(|d| !d.is_null()).cloned()).map_err(|e| e.to_string())?;
            w.dispatch_action(action, cx);
            Ok(Value::Null)
        }),
    ]
}
