# Wiring

| File | Use |
| --- | --- |
| [`display-node.svg`](display-node.svg) | the diagram; source of truth for the rendered formats |
| [`display-node.pdf`](display-node.pdf) | printing, or pinning above the bench |
| [`display-node.png`](display-node.png) | pasting into issues and chat |
| [`display-node.txt`](display-node.txt) | reading in a terminal, grepping, diffing in review |

## These are generated

All four come from `firmware/glucobeacon-fw-display/src/board.rs` by way of
`tools/wiring/generate.py`. Editing them by hand is pointless — the next
regeneration overwrites it — and worse than pointless, because it reintroduces
the failure mode this setup exists to prevent.

```sh
./tools/wiring/render.sh
```

The SVG and the text need only Python. The PDF and PNG additionally need
Chromium; the script finds Playwright's copy or one on `PATH`, and skips those
two formats with a warning if there is none.

CI regenerates the SVG and the text and fails if they differ from what is
committed, so the diagram cannot quietly fall out of step with the firmware.

## Why generated rather than drawn

Wiring diagram v1.0 was drawn by hand. It put the e-paper SPI bus on GPIO 11,
12 and 13, and listed GPIO 8, 9 and 14 as free. On a Heltec V3 those seven pins
are the SX1262 — soldered down, not on a header, not negotiable. Building to
that drawing would have produced a display node whose radio initialises cleanly
and then never receives anything, which is about the least pleasant class of
bug to chase on a bench.

Nothing about the drawing looked wrong. The pins are only obviously taken if
you already know the board, which is exactly the knowledge a diagram is
supposed to carry.

So the numbers now have one home, in the firmware, and the diagram is a view of
it. `generate.py` also refuses to draw a diagram that is wrong: it re-checks
for two peripherals on one pin, for anything landing on the radio, and for pins
that do not exist on an ESP32-S3 — the same assertions
`crates/glucobeacon-display/tests/heltec_board.rs` runs against the firmware.

The original hand-drawn v1.0 is still in the repository root as
`4A1F9E18-7F54-4965-BDE3-941843675C4B.png`, and the "Changed from v1.0" note on
the diagram lists what moved and why.

## Still open

**GPIO0 for the acknowledge button is a strapping pin.** Held down through a
reset — a brown-out, a battery swap, a child leaning on the enclosure — the
ESP32-S3 comes up in its serial bootloader: dark, silent, not alarming. It is
convenient because it parallels the on-board PRG button, so this is a real
trade rather than a mistake, but for a device whose whole job is to make noise
when someone is low it is worth a second thought.
`board::ALTERNATE_ACK_BUTTON` is GPIO47 if the trade is not worth it.

**The pin numbers are from the V3 reference design, not measured.** The checks
above catch pins fighting each other. They cannot catch a pin that is simply
the wrong number — verify against the schematic for your board revision before
the first flash.
