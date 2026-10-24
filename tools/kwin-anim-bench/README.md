# KWin animation benchmark

Measures how smoothly window animations reach the tablet with the Linux host on KDE Plasma 6.
A KWin script plays the same animations every run, and the host trace shows which frames KWin
delivered and which ones the host encoded. Results from this tool are in
[`docs/phase0-findings.md`](../../docs/phase0-findings.md), under "Animation benchmark".

The script runs these events, one every 800 ms:

1. 8 quick-tile previews: the outline is shown, then hidden.
2. 8 maximize/restore cycles of an editor window.
3. 3 s of window moves on a 16 ms timer.

It logs `EZDANIM <kind> <n>` for every event and `EZDTICK <ms>` whenever KWin's main thread ran
a 10 ms timer more than 30 ms late.

## Running

```sh
cargo build --release -p ezd-host
export ANDROID_SERIAL=<tablet serial>   # the ezdisplay app must be installed
tools/kwin-anim-bench/bench.sh base120
tools/kwin-anim-bench/bench.sh base60 --max-refresh-mhz 60000
EZD_DIAG_SKIP_ENCODE=1 tools/kwin-anim-bench/bench.sh kwin-only60 --max-refresh-mhz 60000
tools/kwin-anim-bench/bench.sh --panel eDP-1 panel
```

Arguments after the label go to `ezd-host run`. The host runs for 50 s; the animations start
10 s in, on the output whose name starts with `Virtual`. `--panel <output>` plays them on a real
output without starting the host.

During a run the script moves and maximizes a KWrite window, so leave the desktop alone. The
screen must stay on: KWin stops drawing while the panel is blanked, so the script runs under
`kde-inhibit`.

| Variable | Default | Meaning |
|---|---|---|
| `EZD_HOST` | `target/release/ezd-host` | Host binary |
| `EZD_BENCH_OUT` | `~/.cache/ezd-bench` | Logs and reports |
| `EZD_BENCH_EDITOR` | `kwrite` | Editor whose window is animated |
| `EZD_LOG` | trace for `source`, `encode`, `pw` | Host `RUST_LOG` |
| `EZD_DIAG_SKIP_ENCODE` | unset | The host drops every frame without converting or encoding it, so the run measures KWin alone |
| `EZD_CONVERT_AHEAD` | unset | `1` converts the next frame on its own thread while the current one is encoded and sent, starting so it finishes when the encoder asks for it; `early` starts right after the previous frame was taken. Unset, each frame is converted and encoded in series |

## Output

The run writes `<label>.log` (host), `<label>.events` (script markers) and `<label>.gpu` (GPU
samples) to `EZD_BENCH_OUT`, then prints three reports.

- `report.py`: for each kind of event, the median number of frames KWin delivered and the host
  encoded in the following 500 ms, the delay to the first encoded frame, the longest gap, and
  conversions that took more than 40 ms. For the moves, frame rate and gaps from KWin and
  encoded. Last, the client's end-to-end latency during the test.
- `gpu.py report`: Intel iGPU busy time (1 − RC6 residency) in the 300 ms after each event.
  The frequency in sysfs reads the idle clock while the GPU sleeps, so busy time is the useful
  number.
- `tick.py`: the longest KWin main-thread stall after each kind of event.

`frames.py <log> <events> <kind> [n]` prints the per-frame timeline after the first `n` events of
one kind: frames from KWin, the encode stages and the encoded frames.
