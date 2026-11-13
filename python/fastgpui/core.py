'Drive a GPUI app off screen from Python, through the GPUI debug protocol (GDP) that fastgpui serves'

__all__ = ['GDP', 'Element', 'Elements', 'stop']

import base64, json, os, re, socket, subprocess, time
from pathlib import Path
from PIL import Image

class Element(dict):
    "An element from the last frame: `path`, `role`, `label`, `value`, `visible`, `focused`, `bounds` (x, y, width, height) and so on."
    @property
    def id(self): return self['path'][-1]
    @property
    def bounds(self): return self['bounds']
    @property
    def center(self):
        x, y, w, h = self.bounds
        return x + w/2, y + h/2
    def at(self, dx, dy):
        "The window position offset (`dx`, `dy`) from this element's top-left corner"
        return self.bounds[0] + dx, self.bounds[1] + dy
    def line(self):
        "This element as one Markdown line"
        props = ''.join(f' `{k}`' for k in ('focused', 'checked', 'selected', 'expanded', 'disabled') if self.get(k))
        value = f' value={self["value"]!r}' if self.get('value') else ''
        label = f' "{self["label"]}"' if self.get('label') else ''
        hidden = '' if self.get('visible') else ' (hidden)'
        return f'**{self.get("role") or "-"}**{label} `{self.id}`{value}{props} {[round(o) for o in self.bounds]}{hidden}'
    __repr__ = line

class Elements(list):
    "Elements, displayed as a Markdown tree nested by element path"
    def _tree(self):
        paths = [tuple(e['path']) for e in self]
        def parent(p): return max((q for q in paths if len(q) < len(p) and p[:len(q)] == q), key=len, default=None)
        kids = {}
        for e, p in zip(self, paths): kids.setdefault(parent(p), []).append((e, p))
        def render(p, depth): return [l for e, q in kids.get(p, []) for l in ['  '*depth + '- ' + e.line(), *render(q, depth+1)]]
        return '\n'.join(render(None, 0))
    def _repr_markdown_(self): return self._tree()
    __repr__ = _repr_markdown_
    def grep(self, pattern):
        "The elements whose Markdown line matches regex `pattern`"
        return Elements(e for e in self if re.search(pattern, e.line()))

def _wrap(o):
    if isinstance(o, dict) and 'path' in o and 'bounds' in o: return Element(o)
    if isinstance(o, list) and o and all(isinstance(x, dict) and 'path' in x for x in o): return Elements(map(Element, o))
    return o

def _arg(o): return o.id if isinstance(o, Element) else o

def stop(proc, timeout=10):
    "Terminates `proc`, killing it if it hasn't exited within `timeout` seconds"
    proc.terminate()
    try: proc.wait(timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()

class GDP:
    "A connection to an app serving the GPUI debug protocol. Its methods are the server's commands, listed by `commands`."
    def __init__(self,
        path, # The Unix socket the app serves on
        proc=None, # The app's process, which `close` stops
        log=None, # The file holding the app's output
        timeout=60, # Seconds to wait for the socket
    ):
        self.path, self.proc, self.log = Path(path), proc, log
        deadline = time.monotonic() + timeout
        while True:
            if proc and proc.poll() is not None: raise RuntimeError(f'the app exited with status {proc.returncode}; its output is in {log}')
            try:
                self.sock = socket.socket(socket.AF_UNIX)
                self.sock.connect(str(self.path))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                self.sock.close()
                if time.monotonic() > deadline: raise TimeoutError(f'nothing served {self.path} within {timeout} s')
                time.sleep(0.05)
        self.file = self.sock.makefile('rwb')
        self.specs = {c['name']: c for c in self.call('commands')}

    @classmethod
    def launch(cls,
        project, # The Cargo project to build and run
        *args, # Arguments for the app
        bin=None, # The binary to run, when the project has several
        example=None, # An example to run instead of a binary
        features='gdp', # The cargo features that turn on fastgpui; None for none
        env=None, # Extra environment variables for the app
        build_timeout=1800, # Seconds to allow the build
    ):
        "Builds `project`, starts it serving the protocol on a fresh socket, and connects. Use it in a `with` block, which stops the app."
        project = Path(project).expanduser()
        cmd = ['cargo', 'build', '--message-format=json-render-diagnostics', *(['--features', features] if features else []),
            *(['--example', example] if example else ['--bin', bin] if bin else [])]
        out = subprocess.run(cmd, cwd=project, stdout=subprocess.PIPE, text=True, timeout=build_timeout)
        if out.returncode: raise RuntimeError(f'`{" ".join(cmd)}` failed in {project}')
        msgs = [json.loads(l) for l in out.stdout.splitlines() if l.startswith('{')]
        kind = 'example' if example else 'bin'
        exes = [m['executable'] for m in msgs if m.get('reason') == 'compiler-artifact' and m.get('executable') and kind in m['target']['kind']]
        if len(exes) != 1: raise RuntimeError(f'expected one {kind} from the build, found {exes}; pass `bin` or `example`')
        rt = Path(os.environ.get('XDG_RUNTIME_DIR', '/tmp'))
        path = rt/f'gdp-{os.getpid()}-{time.monotonic_ns()}.sock'
        log = path.with_suffix('.log')
        with open(log, 'w') as f:
            proc = subprocess.Popen([exes[0], *args], cwd=project, stdout=f, stderr=f, env={**os.environ, **(env or {}), 'GDP_SOCKET': str(path)})
        try: return cls(path, proc, log)
        except BaseException:
            stop(proc)
            raise

    def call(self, cmd, **kwargs):
        "Sends command `cmd` with fields `kwargs` and returns its result. A failed command, such as a missing element, raises `RuntimeError`."
        req = {'cmd': cmd, **{k: _arg(v) for k, v in kwargs.items() if v is not None}}
        self.file.write(json.dumps(req).encode() + b'\n')
        self.file.flush()
        line = self.file.readline()
        if not line: raise ConnectionError(f'the app closed the connection; its output is in {self.log}')
        reply = json.loads(line)
        if not reply['ok']: raise RuntimeError(reply['error'])
        return _wrap(reply['result'])

    def __getattr__(self, name):
        spec = self.__dict__.get('specs', {}).get(name)
        if spec is None: raise AttributeError(name)
        params = [p for p in spec['params'] if p != 'within']
        def f(*args, **kwargs):
            if len(args) > len(params): raise TypeError(f'{name} takes at most {len(params)} positional arguments: {params}')
            return self.call(name, **dict(zip(params, args)), **kwargs)
        f.__name__, f.__doc__ = name, f'{spec["doc"]}. Fields: {", ".join(spec["params"]) or "none"}; every command also takes `window`.'
        return f

    def __dir__(self): return sorted({*super().__dir__(), *self.specs})

    def screenshot(self, window=None) -> Image.Image:
        "The window as rendered off screen"
        r = self.call('screenshot', window=window)
        return Image.frombytes('RGBA', (r['width'], r['height']), base64.b64decode(r['rgba']))

    def _point(self, o): return self.find(o).center if isinstance(o, str) else o.center if isinstance(o, Element) else o

    def drag(self,
        frm, # Where to press: an element, an element id (its centre), or an (x, y) window position
        to, # Where to release, in the same forms
        steps=8, # Pointer moves between press and release, each followed by the server's settle
        button='left',
        window=None,
    ):
        "Drags from `frm` to `to` one pointer step at a time, so the app handles each step as it would a real drag"
        (x0, y0), (x1, y1) = self._point(frm), self._point(to)
        self.pointer_move(x0, y0, window=window)
        self.pointer_down(x0, y0, button, window=window)
        for i in range(1, steps+1): self.pointer_move(x0 + (x1-x0)*i/steps, y0 + (y1-y0)*i/steps, button, window=window)
        self.pointer_up(x1, y1, button, window=window)

    def wait_for(self,
        check, # A function returning a truthy value when done, or an element id that must be present
        timeout=10, # Seconds of real time to wait
        every=0.02, # Seconds to sleep between checks
    ):
        "Polls `check` in real time and returns its truthy result. Between checks, GPUI's clock advances by the real time that passed."
        if isinstance(check, str): check = lambda id=check: self.try_find(id)
        start = last = time.monotonic()
        while True:
            now = time.monotonic()
            self.advance(int((now - last) * 1000))
            last = now
            if r := check(): return r
            if now - start > timeout: raise TimeoutError(f'{check} stayed false for {timeout} s')
            time.sleep(every)

    def close(self):
        "Disconnects, and stops the app if `launch` started it, removing its socket and log"
        self.file.close()
        self.sock.close()
        if self.proc:
            stop(self.proc)
            self.path.unlink(missing_ok=True)
            self.log.unlink(missing_ok=True)

    def __enter__(self): return self
    def __exit__(self, *exc): self.close()
