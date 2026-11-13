from pathlib import Path
from fastgpui import GDP

def test_drive_the_counter():
    "The counter example over the socket: a click, a deferred count, a drag-select of the focused field, Ctrl+C and a screenshot"
    with GDP.launch(Path(__file__).parent.parent, example='counter', features=None) as app:
        assert app.find('increment')['label'] == 'Count: 0'
        app.click('increment')
        assert app.find('increment')['label'] == 'Count: 1'
        assert app.elements().grep('`name`')
        app.click('name')
        name = app.find('name')
        x, y, w, h = name.bounds
        app.drag(name.at(w-16, h/2), name.at(2, h/2))
        app.press('ctrl-c')
        assert app.clipboard() == 'hello world'
        assert app.screenshot().size[0] > 0
