# fastgpui

fastgpui lets a script drive a GPUI app off screen, the way fastcdp drives Chrome through its debugging port. It is for checking UI work in a real app, from Python, without a window on your desktop. The app runs in GPUI's headless app with the real text system and renderer, and its windows are built by its own start-up code. Commands are gpui-kit's test operations: find elements, click, drag, type and press keys, read the clipboard, and take screenshots.

## Add it to an app

Make fastgpui an optional dependency, turned on by a `gdp` ("GPUI debug protocol") feature:

```toml
[dependencies]
fastgpui = { path = "../fastgpui", optional = true }

[features]
gdp = ["dep:fastgpui"]
```

fastgpui turns on gpui-kit's `test-support` feature, so a build without `gdp` contains neither fastgpui nor gpui-kit's test support.

In `main`, give the start-up closure a name and serve it when `GDP_SOCKET` is set:

```rust
fn main() {
    let start = |cx: &mut gpui_kit::App| { gpui_kit::init(cx); /* open windows */ };
    #[cfg(feature = "gdp")]
    if fastgpui::requested() { return fastgpui::serve(start) }
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(start);
}
```

Elements are reachable when gpui-kit components register them, or when the app gives them an `.id(...)` or `.test_support()`. An app can add its own commands with `fastgpui::Server::new(start).command(name, doc, params, run).serve()`, where `run` gets the target window and the request's fields. `examples/counter.rs` is a complete app.

## Drive it from Python

Install the Python package into the environment you script from by running `maturin develop` in this folder with that environment active. Then:

```python
from fastgpui import GDP
with GDP.launch('~/aai-ws/myapp', 'an-app-argument') as app:
    app.elements()                    # the last frame's elements, as a Markdown tree
    app.click('save')
    app.wait_for(lambda: app.find('status')['label'] == 'Saved')
    app.drag('handle', 'drop-zone')   # from one element's centre to another's, one pointer step at a time
    app.press('ctrl-c')
    app.clipboard()
    app.screenshot()                  # a PIL image
```

`GDP.launch` builds the project with `--features gdp`, starts its binary with `GDP_SOCKET` naming a fresh socket in `$XDG_RUNTIME_DIR`, and connects. Leaving the `with` block stops the app and removes the socket and its log. `GDP` builds one method per command from the server's `commands` list, with the command's doc, so an app's own commands need no Python code.

After each command the server runs every ready task and draws a frame. It never waits for work that isn't ready, such as a network reply or a timer. `wait_for` polls in real time and advances GPUI's simulated clock by the time that passed, so timers fire as they would in the app. `drag` sends one pointer step per command.

## The protocol

The app answers JSON requests, one per line, on the Unix socket that `GDP_SOCKET` names. A request is `{"cmd": NAME, ...fields}`, with an optional `window` id. The reply is `{"ok": true, "result": ...}` or `{"ok": false, "error": ...}`; a gpui-kit panic, such as a missing element, becomes an error and the app keeps serving. `{"cmd": "commands"}` lists every command with its doc and fields.

## Development

```bash
cargo test
pytest -q
```

See `DEV.md` for the layout, build profiles and releases.
