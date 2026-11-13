"""Drive a GPUI app off screen to check UI work, as fastcdp drives Chrome: find elements, act through real input, and verify results.

The app must opt in. Its `Cargo.toml` makes `fastgpui` an optional dependency turned on by a `gdp` feature, and its `main` serves the protocol when `fastgpui::requested()` (see the fastgpui README). `GDP.launch(project, *args)` builds the project with `--features gdp`, starts it with `GDP_SOCKET` set, and connects. The app runs in GPUI's headless app with the real text system and renderer: nothing appears on the desktop, and its windows are built by its own start-up code.

    from fastgpui import GDP
    with GDP.launch('~/aai-ws/myapp', 'some-arg') as app:
        app.elements()                     # the last frame's elements, as a Markdown tree; `.grep('Button')` filters it
        app.click('save')                  # element ids are the app's `.id(...)` values
        app.wait_for(lambda: app.find('status')['label'] == 'Saved')
        app.screenshot()                   # a PIL image

# Discover

The server describes its commands, including any the app adds, and `GDP` has one method per command: `dir(app)`, `doc(app.click)`, `app.commands()`. Positional arguments fill a command's fields in order; `within='parent'` or a list of ids scopes element commands to nested GPUI identity scopes; `window=` targets a window from `app.windows()`. The commands follow gpui-kit's test API: `find`, `try_find`, `find_all`, `find_by_label`, `find_all_by_role`, `elements`, `texts`, `click`, `click_at`, `double_click`, `right_click`, `hover`, `scroll`, `press`, `input`, the single steps `pointer_down`, `pointer_move`, `pointer_up`, `key_down`, `key_up` and `change_modifiers`, and `clipboard`, `actions`, `action`, `screenshot` and `advance`.

# Read, act, verify

Inspect `elements()` before acting, then act, then check an observable result: an element's `label`, `value` or `focused`, the clipboard after `press('ctrl-c')`, or a screenshot. Element queries see only elements the app registers, through gpui-kit components or `.id(...)`/`.test_support()`; `label` and `value` are accessibility properties, not drawn text, and `texts()` returns only text that gpui-kit components register.

After every command the server runs each ready task and draws a frame, so synchronous effects are visible immediately. It never waits for work that isn't ready, such as a network reply or a timer. For those, use `wait_for(check)`, which polls in real time and advances GPUI's simulated clock by the time that passed, so timers fire as they would in the app. Use `drag(frm, to)` for drags: it sends one pointer step per command, so the app sees each step settle, as in a real drag.

A failed command raises `RuntimeError` with gpui-kit's message, such as a missing element and the registered paths, and the app keeps running. The app's own output is in `app.log`.
"""

from fastgpui.core import *

__all__ = ['GDP', 'Element', 'Elements']
