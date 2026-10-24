# Input test page

Shows what the browser receives from the injected input and logs every event to a JSONL file, so
the values can be checked as text: pointer type, pressure, tilt, twist, buttons, simultaneous
touch contacts, wheel events (with Ctrl for zoom) and context menus. Pen and touch strokes are
drawn on the page, with the pen's width following the pressure.

## Running

```sh
python3 tools/input-test/serve.py            # http://127.0.0.1:8765/
```

Open the page in Firefox or Chromium, move the window to the tablet's output and make it full
screen (F11), so that the page's normalized coordinates (`nx`, `ny`) match the stream's. The
right-hand panel scrolls natively, for two-finger scrolling. `C` clears the drawing and `L`
pauses logging.

The log goes to `~/.cache/ezd-input-test/<time>.jsonl` (`--out` changes it). Each line is one
event with the browser's timestamp (`t`, ms since the epoch), the dispatch lag (`lag`, ms) and the
server's receive time (`srv`). Summarize a log with:

```sh
python3 tools/input-test/serve.py --summary ~/.cache/ezd-input-test/<time>.jsonl
```

The summary lists event counts, move rates, pressure and tilt ranges, the buttons seen, the
largest number of simultaneous contacts and the wheel totals.
