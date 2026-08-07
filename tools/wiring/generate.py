#!/usr/bin/env python3
"""Draw the display node's wiring from the firmware's own pin map.

Wiring diagram v1.0 was drawn by hand and put the e-paper bus on top of the
soldered-down SX1262. That is not a mistake a careful reviewer catches — the
pins look free unless you happen to know the board — so this reads the numbers
out of ``board.rs`` instead of restating them. The picture cannot disagree with
the firmware, because there is only one copy of the fact.

Emits an SVG and an ASCII rendering. See ``tools/wiring/render.sh`` for the PDF.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from xml.sax.saxutils import escape

REPO = Path(__file__).resolve().parents[2]
BOARD_RS = REPO / "firmware/glucobeacon-fw-display/src/board.rs"
OUT_DIR = REPO / "docs/wiring"

# ---------------------------------------------------------------- parsing


def parse_board(source: str) -> dict[str, int]:
    """Pull ``pub const NAME: u8 = N;`` out of board.rs, module-qualified.

    A brace-counting scan rather than a bare regex, so a constant inside
    ``mod lora`` comes back as ``lora.SCK`` and cannot be confused with the
    e-paper's ``SCK``.
    """
    pins: dict[str, int] = {}
    module: str | None = None
    depth = 0

    # Ignore the test module: its arrays repeat the constants.
    source = source.split("#[cfg(test)]")[0]

    for line in source.splitlines():
        stripped = line.strip()

        opened = re.match(r"pub mod (\w+)\s*\{", stripped)
        if opened:
            module = opened.group(1)
            depth = 1
            continue

        if module is not None:
            depth += line.count("{") - line.count("}")
            if depth <= 0:
                module = None
                continue

        const = re.match(r"pub const (\w+): u8 = (\d+);", stripped)
        if const:
            name, value = const.group(1), int(const.group(2))
            pins[f"{module}.{name}" if module else name] = value

    return pins


def parse_radio(source: str) -> dict[str, str]:
    """Pull the radio settings, so the diagram states the same link the
    firmware brings up."""
    settings = {}
    region = re.search(r"pub const REGION: Region = Region::(\w+);", source)
    if region:
        settings["region"] = {"Us915": "US915 (902-928 MHz)", "Eu868": "EU868 (863-870 MHz)"}.get(
            region.group(1), region.group(1)
        )
    sf = re.search(r"spreading_factor: SpreadingFactor::Sf(\d+)", source)
    if sf:
        settings["sf"] = f"SF{sf.group(1)}"
    bw = re.search(r"bandwidth: Bandwidth::Bw(\d+)", source)
    if bw:
        settings["bw"] = f"{bw.group(1)} kHz"
    frame = re.search(r"pub const MAX_FRAME_FOR_DWELL: usize = (\d+);", source)
    if frame:
        settings["max_frame"] = f"{frame.group(1)} bytes"
    return settings


# ---------------------------------------------------------------- model


@dataclass
class Pin:
    """One connection between the MCU and something else."""

    signal: str  # what the peripheral calls it
    key: str  # lookup into the parsed pin map
    note: str = ""
    net: str = "ctrl"  # colour class


@dataclass
class Block:
    """A peripheral, and the pins that reach it."""

    title: str
    subtitle: str
    pins: list[Pin]
    fixed: bool = False  # soldered to the module; pins are not negotiable
    warn: str = ""
    height_pad: int = 0
    y0: float = field(default=0.0, init=False)
    y1: float = field(default=0.0, init=False)


LEFT = [
    Block(
        "SX1262 LoRa radio",
        "on-module, SPI2",
        [
            Pin("NSS", "lora.NSS", "chip select", "spi2"),
            Pin("SCK", "lora.SCK", "", "spi2"),
            Pin("MOSI", "lora.MOSI", "", "spi2"),
            Pin("MISO", "lora.MISO", "", "spi2"),
            Pin("RST", "lora.RESET", "active low", "ctrl"),
            Pin("BUSY", "lora.BUSY", "high while working", "ctrl"),
            Pin("DIO1", "lora.DIO1", "tx/rx done irq", "ctrl"),
        ],
        fixed=True,
        warn="Soldered to the module. These seven pins are not available for anything else.",
    ),
    Block(
        "SSD1306 OLED",
        "on-module, I2C, 128x64",
        [
            Pin("SDA", "oled.SDA", "", "i2c"),
            Pin("SCL", "oled.SCL", "", "i2c"),
            Pin("RST", "oled.RESET", "", "ctrl"),
        ],
        fixed=True,
        warn="Not the product display, but the only one you have until the e-paper works.",
    ),
]

RIGHT = [
    Block(
        "Waveshare 7.5in e-Paper HAT",
        "800x480 mono, UC8179, SPI3",
        [
            Pin("CS", "epaper.CS", "chip select", "spi3"),
            Pin("SCK", "epaper.SCK", "", "spi3"),
            Pin("DIN", "epaper.MOSI", "MOSI; panel is write-only", "spi3"),
            Pin("DC", "epaper.DC", "low = command", "ctrl"),
            Pin("RST", "epaper.RESET", "active low", "ctrl"),
            Pin("BUSY", "epaper.BUSY", "LOW while refreshing", "ctrl"),
        ],
        warn="No MISO: the panel never talks back. BUSY polarity is inverted vs most e-paper.",
    ),
    Block(
        "SFM-27-W buzzer",
        "active buzzer, via MOSFET module #1",
        [Pin("SIG", "BUZZER", "active high", "ctrl")],
        warn="Drive LOW at boot. An active buzzer makes its own tone: no PWM needed.",
    ),
    Block(
        "Arcade button LED ring",
        "via MOSFET module #2",
        [Pin("SIG", "BUTTON_LED", "active high", "ctrl")],
        warn="Drive LOW at boot.",
    ),
    Block(
        "Acknowledge button",
        "momentary, to GND",
        [Pin("SW", "ACK_BUTTON", "active low, pull-up", "ctrl")],
        warn="Not GPIO0: held through a reset, a strapping pin leaves the board "
             "in its bootloader — dark, silent, not alarming.",
    ),
]

# ---------------------------------------------------------------- geometry

W, H = 1900, 1240
HEADER_H = 96
MCU_X, MCU_W = 610, 300
MCU_Y, MCU_H = 168, 780
ROW_H = 46
GROUP_GAP = 26
BLOCK_W = 300
LEFT_X = 96
RIGHT_X = MCU_X + MCU_W + 210
SIDEBAR_X = 1490

NET_COLOURS = {
    "spi2": "#7c3aed",
    "spi3": "#0369a1",
    "i2c": "#0f766e",
    "ctrl": "#b45309",
}

POWER_TEXT = (
    "USB-C 5 V (2 A) and a 3.7 V LiPo meet at a power-path / OR-ing stage. "
    "The e-Paper HAT and both MOSFET modules run from 5 V; the Heltec regulates "
    "its own 3.3 V. All grounds common. Do not power the board without the "
    "antenna attached."
)

CHANGES_TEXT = (
    "v1.1 — e-Paper SCK/MOSI/MISO were on GPIO12/11/13 and DC/CS on GPIO17/18: the "
    "first three are the SX1262, the last two the on-board OLED, so the bus moved to "
    "free pins. e-Paper MISO dropped, the panel is write-only. GPIO22/23 were listed "
    "as reserve; neither exists on an ESP32-S3.   "
    "v1.2 — the acknowledge button moved off GPIO0 and the e-Paper clock off GPIO3. "
    "Both are strapping pins: held or pulled at reset they change how the chip boots, "
    "and a board stuck in its bootloader is dark, silent and not alarming."
)

INK = "#111827"
MUTED = "#6b7280"
RULE = "#d1d5db"
FIXED_FILL = "#f3f4f6"
FIXED_STROKE = "#9ca3af"


def wrap(text: str, width: int) -> list[str]:
    """Greedy wrap. SVG has no flow text, so long notes have to be broken up
    here or they run out of their box and over whatever is next to it."""
    words, lines, current = text.split(), [], ""
    for word in words:
        candidate = f"{current} {word}".strip()
        if len(candidate) > width and current:
            lines.append(current)
            current = word
        else:
            current = candidate
    if current:
        lines.append(current)
    return lines


def layout(blocks: list[Block], top: float) -> float:
    """Stack blocks downward, one row per pin. Returns the bottom edge."""
    y = top
    for block in blocks:
        block.y0 = y
        warn_lines = len(wrap(block.warn, 52)) if block.warn else 0
        y += 34 + len(block.pins) * ROW_H + 10 + warn_lines * 12 + block.height_pad
        block.y1 = y
        y += GROUP_GAP
    return y


def row_y(block: Block, index: int) -> float:
    return block.y0 + 34 + index * ROW_H + ROW_H / 2


def footer_layout(top: float) -> tuple[dict[str, float], list[str], list[str], float]:
    """Where each footer line goes, and how far down the last one reaches.

    Measured rather than assumed: the v1.2 note is longer than the v1.1 one was,
    and a hardcoded footer height silently cropped its last line.
    """
    power = wrap(POWER_TEXT, 118)
    changes = wrap(CHANGES_TEXT, 128)
    y = {"rule": top, "power_head": top + 22, "power": top + 44}
    after_power = y["power"] + (len(power) - 1) * 16
    y["changes_head"] = after_power + 38
    y["changes"] = y["changes_head"] + 20
    bottom = y["changes"] + (len(changes) - 1) * 15 + 28
    return y, power, changes, bottom


def footer_top() -> float:
    return MCU_Y + MCU_H + 62


def fit_to_content() -> None:
    """Lay both columns out, then size the MCU and the canvas around them.

    These were fixed numbers until a wrapped warning pushed the acknowledge
    button's stub past the bottom of the chip it connects to — a diagram that
    is drawn wrong is worse than one that is missing.
    """
    global MCU_H, H

    layout(LEFT, MCU_Y + 96)
    layout(RIGHT, MCU_Y + 96)

    content_bottom = max(LEFT[-1].y1, RIGHT[-1].y1)
    MCU_H = content_bottom - MCU_Y + 20
    *_, footer_bottom = footer_layout(footer_top())
    H = int(footer_bottom)


# ---------------------------------------------------------------- svg


class Svg:
    def __init__(self) -> None:
        self.parts: list[str] = []

    def add(self, markup: str) -> None:
        self.parts.append(markup)

    def text(
        self,
        x: float,
        y: float,
        content: str,
        size: float = 13,
        weight: str = "normal",
        fill: str = INK,
        anchor: str = "start",
        mono: bool = False,
        opacity: float = 1.0,
    ) -> None:
        family = (
            "ui-monospace, 'DejaVu Sans Mono', Menlo, Consolas, monospace"
            if mono
            else "'DejaVu Sans', Helvetica, Arial, sans-serif"
        )
        self.add(
            f'<text x="{x:.1f}" y="{y:.1f}" font-family="{family}" font-size="{size}" '
            f'font-weight="{weight}" fill="{fill}" text-anchor="{anchor}" '
            f'opacity="{opacity}">{escape(content)}</text>'
        )

    def rect(
        self,
        x: float,
        y: float,
        w: float,
        h: float,
        fill: str = "none",
        stroke: str = INK,
        width: float = 1.4,
        rx: float = 8,
        dash: str = "",
    ) -> None:
        d = f' stroke-dasharray="{dash}"' if dash else ""
        self.add(
            f'<rect x="{x:.1f}" y="{y:.1f}" width="{w:.1f}" height="{h:.1f}" rx="{rx}" '
            f'fill="{fill}" stroke="{stroke}" stroke-width="{width}"{d}/>'
        )

    def line(self, x1: float, y1: float, x2: float, y2: float, stroke: str, width: float = 2.0) -> None:
        self.add(
            f'<line x1="{x1:.1f}" y1="{y1:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" '
            f'stroke="{stroke}" stroke-width="{width}" stroke-linecap="round"/>'
        )

    def dot(self, x: float, y: float, fill: str, r: float = 3.4) -> None:
        self.add(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{r}" fill="{fill}"/>')

    def render(self) -> str:
        return (
            f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
            f'viewBox="0 0 {W} {H}">\n'
            f'<rect width="{W}" height="{H}" fill="#ffffff"/>\n'
            + "\n".join(self.parts)
            + "\n</svg>\n"
        )


def gpio_chip(svg: Svg, x: float, y: float, number: int, colour: str, anchor: str) -> float:
    """A small rounded tag holding the GPIO number, on the MCU edge.

    Returns the x coordinate the wire should leave from.
    """
    w, h = 62, 24
    bx = x if anchor == "left" else x - w
    svg.rect(bx, y - h / 2, w, h, fill="#ffffff", stroke=colour, width=1.6, rx=6)
    svg.text(bx + w / 2, y + 4.5, f"IO{number}", size=12.5, weight="bold", fill=colour,
             anchor="middle", mono=True)
    return bx if anchor == "right" else bx + w


def draw_block(svg: Svg, block: Block, x: float, pins: dict[str, int], side: str) -> None:
    height = block.y1 - block.y0
    stroke = FIXED_STROKE if block.fixed else INK
    svg.rect(x, block.y0, BLOCK_W, height,
             fill=FIXED_FILL if block.fixed else "#ffffff",
             stroke=stroke, width=1.6, dash="6 4" if block.fixed else "")

    svg.text(x + 14, block.y0 + 21, block.title, size=13.5, weight="bold",
             fill=INK if not block.fixed else "#374151")
    svg.text(x + 14, block.y0 + 37, block.subtitle, size=10.5, fill=MUTED)

    if block.fixed:
        svg.rect(x + BLOCK_W - 62, block.y0 + 8, 52, 17, fill="#e5e7eb", stroke="none", rx=8)
        svg.text(x + BLOCK_W - 36, block.y0 + 20, "FIXED", size=9.5, weight="bold",
                 fill="#374151", anchor="middle")

    for index, pin in enumerate(block.pins):
        y = row_y(block, index)
        colour = NET_COLOURS[pin.net]
        label_x = x + BLOCK_W - 14 if side == "left" else x + 14
        anchor = "end" if side == "left" else "start"
        svg.text(label_x, y - 2, pin.signal, size=12.5, weight="bold", fill=colour, anchor=anchor,
                 mono=True)
        if pin.note:
            svg.text(label_x, y + 12, pin.note, size=9.5, fill=MUTED, anchor=anchor)
        edge = x + BLOCK_W if side == "left" else x
        svg.dot(edge, y, colour)

    if block.warn:
        lines = wrap(block.warn, 52)
        for offset, line in enumerate(lines):
            svg.text(x + 14, block.y1 - 8 - (len(lines) - 1 - offset) * 12, line,
                     size=9.5, fill="#9a3412")


def build_svg(pins: dict[str, int], radio: dict[str, str], version: str) -> str:
    svg = Svg()

    # ---- header
    svg.text(48, 46, "GlucoBeacon — Display Node", size=27, weight="bold")
    svg.text(48, 70, "Heltec WiFi LoRa 32 V3  ·  ESP32-S3FN8 + SX1262  ·  "
                     "Waveshare 7.5in e-Paper", size=13, fill=MUTED)
    svg.text(W - 48, 40, f"wiring {version}", size=13, weight="bold", fill=MUTED, anchor="end")
    svg.text(W - 48, 60, "generated from firmware/glucobeacon-fw-display/src/board.rs",
             size=10.5, fill=MUTED, anchor="end", mono=True)
    svg.text(W - 48, 76, "regenerate with tools/wiring/render.sh — do not edit by hand",
             size=10.5, fill=MUTED, anchor="end", mono=True)
    svg.line(48, HEADER_H, W - 48, HEADER_H, RULE, 1.4)

    # ---- MCU
    svg.rect(MCU_X, MCU_Y, MCU_W, MCU_H, fill="#ffffff", stroke=INK, width=2.2, rx=12)
    svg.text(MCU_X + MCU_W / 2, MCU_Y + 30, "Heltec WiFi LoRa 32 V3", size=15, weight="bold",
             anchor="middle")
    svg.text(MCU_X + MCU_W / 2, MCU_Y + 50, "ESP32-S3FN8 · 512 KB SRAM · 8 MB flash",
             size=10.5, fill=MUTED, anchor="middle")
    svg.text(MCU_X + MCU_W / 2, MCU_Y + 66, "all GPIO are 3.3 V", size=10.5, fill=MUTED,
             anchor="middle")
    svg.line(MCU_X + 20, MCU_Y + 78, MCU_X + MCU_W - 20, MCU_Y + 78, RULE, 1.2)

    # ---- blocks and wires
    for side, blocks, bx in (("left", LEFT, LEFT_X), ("right", RIGHT, RIGHT_X)):
        for block in blocks:
            draw_block(svg, block, bx, pins, side)
            for index, pin in enumerate(block.pins):
                y = row_y(block, index)
                colour = NET_COLOURS[pin.net]
                number = pins[pin.key]
                if side == "left":
                    start = bx + BLOCK_W
                    chip_edge = gpio_chip(svg, MCU_X, y, number, colour, "right")
                    svg.line(start, y, chip_edge, y, colour)
                else:
                    end = bx
                    chip_edge = gpio_chip(svg, MCU_X + MCU_W, y, number, colour, "left")
                    svg.line(chip_edge, y, end, y, colour)

    # ---- sidebar
    sx = SIDEBAR_X
    sw = W - sx - 48
    svg.text(sx, HEADER_H + 36, "Nets", size=14, weight="bold")
    ny = HEADER_H + 58
    for label, key in (
        ("SPI2 — radio (on-module)", "spi2"),
        ("SPI3 — e-paper", "spi3"),
        ("I2C — OLED (on-module)", "i2c"),
        ("control / GPIO", "ctrl"),
    ):
        svg.line(sx, ny - 4, sx + 26, ny - 4, NET_COLOURS[key], 3)
        svg.text(sx + 34, ny, label, size=11)
        ny += 20

    ny += 14
    svg.text(sx, ny, "Radio link", size=14, weight="bold")
    ny += 20
    for label, value in (
        ("region", radio.get("region", "?")),
        ("spreading factor", radio.get("sf", "?")),
        ("bandwidth", radio.get("bw", "?")),
        ("max frame", radio.get("max_frame", "?")),
    ):
        svg.text(sx, ny, label, size=11, fill=MUTED)
        svg.text(sx + sw, ny, value, size=11, weight="bold", anchor="end", mono=True)
        ny += 18
    svg.text(sx, ny + 4, "Both ends must match exactly. A mismatch", size=9.5, fill=MUTED)
    svg.text(sx, ny + 16, "is a silent link, not a weak one.", size=9.5, fill=MUTED)
    ny += 40

    svg.text(sx, ny, "Pin map", size=14, weight="bold")
    ny += 8
    ordered = (
        [("SX1262", p) for p in LEFT[0].pins]
        + [("OLED", p) for p in LEFT[1].pins]
        + [("e-Paper", p) for p in RIGHT[0].pins]
        + [("Buzzer", RIGHT[1].pins[0]), ("Button LED", RIGHT[2].pins[0]),
           ("Button", RIGHT[3].pins[0])]
    )
    for owner, pin in ordered:
        ny += 17
        svg.text(sx, ny, f"{owner} {pin.signal}", size=10.5, fill=INK)
        svg.text(sx + sw, ny, f"GPIO{pins[pin.key]}", size=10.5, weight="bold", anchor="end",
                 mono=True)

    ny += 26
    svg.text(sx, ny, "Free / reserve", size=14, weight="bold")
    ny += 18
    used = set(pins[p.key] for _, p in ordered) | {
        pins[k] for k in ("VEXT_CONTROL", "BOARD_LED", "VBAT_ADC", "VBAT_ENABLE") if k in pins
    }
    # GPIO19/20 are USB, 43/44 the UART, 26-32 the SiP flash, 22-25 absent.
    reserved = set(range(19, 21)) | set(range(22, 33)) | {43, 44}
    free = [n for n in range(0, 49) if n not in used and n not in reserved]
    for line in wrap(", ".join(f"IO{n}" for n in free), 40):
        svg.text(sx, ny, line, size=10, fill=MUTED, mono=True)
        ny += 14
    ny += 4
    svg.text(sx, ny, "GPIO22-25 do not exist on an S3;", size=9.5, fill=MUTED)
    svg.text(sx, ny + 12, "26-32 are the SiP flash.", size=9.5, fill=MUTED)

    # ---- footer: power + provenance
    y, power_lines, change_lines, _ = footer_layout(footer_top())
    svg.line(48, y["rule"], W - 48, y["rule"], RULE, 1.4)
    svg.text(48, y["power_head"], "Power", size=14, weight="bold")
    for offset, line in enumerate(power_lines):
        svg.text(48, y["power"] + offset * 16, line, size=11)

    svg.text(48, y["changes_head"], "Changed from the hand-drawn v1.0", size=13,
             weight="bold", fill="#9a3412")
    for offset, line in enumerate(change_lines):
        svg.text(48, y["changes"] + offset * 15, line, size=10.5, fill="#9a3412")

    return svg.render()


# ---------------------------------------------------------------- ascii


def build_ascii(pins: dict[str, int], radio: dict[str, str], version: str) -> str:
    def wire(signal: str, key: str, note: str) -> str:
        return f"  {signal:<6} {'GPIO' + str(pins[key]):>7}   {note}"

    out: list[str] = []
    add = out.append

    add("GlucoBeacon - Display Node")
    add(f"Heltec WiFi LoRa 32 V3 (ESP32-S3FN8 + SX1262) - wiring {version}")
    add("")
    add("Generated from firmware/glucobeacon-fw-display/src/board.rs by")
    add("tools/wiring/generate.py. Do not edit by hand.")
    add("")
    add("=" * 78)
    add("")
    add("   ON-MODULE (pins fixed by the board)      EXTERNAL (your wiring)")
    add("")
    add("   +----------------------+                 +-------------------------+")
    add("   |  SX1262 LoRa radio   |<-- SPI2 --+     |  Waveshare 7.5in ePaper |")
    add("   |  FIXED, GPIO 8-14    |           |     |  800x480 mono, UC8179   |")
    add("   +----------------------+           |     +-------------------------+")
    add("                                      |                  ^")
    add("   +----------------------+           |            SPI3  |")
    add("   |  SSD1306 OLED 128x64 |<-- I2C -+ |                  |")
    add("   |  FIXED, GPIO 17/18/21|         | |                  |")
    add("   +----------------------+         | |                  |")
    add("                                    v v                  |")
    add("                            +--------------------+       |")
    add("                            |  Heltec V3         |-------+")
    add("                            |  ESP32-S3FN8       |")
    add("                            |  512 KB / 8 MB     |-------+")
    add("                            +--------------------+       |")
    add("                                                   GPIO  |")
    add("                                                         v")
    add("                                     +---------------------------+")
    add("                                     |  buzzer      (MOSFET #1)  |")
    add("                                     |  button LED  (MOSFET #2)  |")
    add("                                     |  acknowledge button       |")
    add("                                     +---------------------------+")
    add("")
    add("-" * 78)
    add("SX1262 LoRa radio            [FIXED - soldered to the module]")
    add("-" * 78)
    for pin in LEFT[0].pins:
        add(wire(pin.signal, pin.key, pin.note))
    add("  These seven pins are not available for anything else. Wiring another")
    add("  peripheral onto them takes the radio down with it.")
    add("")
    add("-" * 78)
    add("SSD1306 OLED, 128x64         [FIXED - soldered to the module]")
    add("-" * 78)
    for pin in LEFT[1].pins:
        add(wire(pin.signal, pin.key, pin.note))
    add("  Not the product display, but the only one you have until the e-paper works.")
    add("")
    add("-" * 78)
    add("Waveshare 7.5in e-Paper HAT (800x480 mono, UC8179, SPI3)")
    add("-" * 78)
    for pin in RIGHT[0].pins:
        add(wire(pin.signal, pin.key, pin.note))
    add("  No MISO: the panel never talks back.")
    add("  BUSY is LOW while refreshing - the opposite of most e-paper controllers.")
    add("  The controller wants 0 for black; the framebuffer uses 1. The driver inverts.")
    add("")
    add("-" * 78)
    add("Buzzer, button LED, acknowledge button")
    add("-" * 78)
    add(wire("BUZZ", "BUZZER", "MOSFET #1 SIG, active high"))
    add(wire("LED", "BUTTON_LED", "MOSFET #2 SIG, active high"))
    add(wire("SW", "ACK_BUTTON", "to GND, active low, internal pull-up"))
    add("  Drive the buzzer and LED LOW at boot, or the buzzer shrieks through resets.")
    add("  An SFM-27-W is an ACTIVE buzzer: on/off, no PWM needed.")
    add("")
    add(f"  The button is on GPIO{pins['ACK_BUTTON']}, deliberately NOT GPIO{pins['PRG_BUTTON']}.")
    add("  GPIO0/3/45/46 are sampled at reset. A button held down through a reset on")
    add("  GPIO0 leaves the board in its serial bootloader: dark, silent, not alarming,")
    add("  with nothing to say what happened. The on-board PRG button is still on")
    add(f"  GPIO{pins['PRG_BUTTON']} if a spare input helps during bring-up.")
    add("")
    add("-" * 78)
    add("Radio link (both ends must match exactly)")
    add("-" * 78)
    add(f"  region              {radio.get('region', '?')}")
    add(f"  spreading factor    {radio.get('sf', '?')}")
    add(f"  bandwidth           {radio.get('bw', '?')}")
    add(f"  max frame           {radio.get('max_frame', '?')}")
    add("  A mismatch in any field is a silent link, not a weak one.")
    add("")
    add("-" * 78)
    add("Power")
    add("-" * 78)
    add("  USB-C 5 V (2 A) ---+")
    add("                     +--> power path / OR-ing --> 5 V rail --> e-Paper HAT,")
    add("  3.7 V LiPo --------+                                        MOSFET modules")
    add("                                                 3.3 V rail -> Heltec (internal)")
    add("  All grounds common. Do not power the board without the antenna attached.")
    add("")
    add("-" * 78)
    add("Changed from wiring diagram v1.0")
    add("-" * 78)
    add("  * e-Paper SCK/MOSI/MISO were GPIO12/11/13 - those are the SX1262's RST,")
    add("    MISO and BUSY. The bus has moved to free pins.")
    add("  * e-Paper DC/CS were GPIO17/18 - those are the on-board OLED's I2C pins.")
    add("  * e-Paper MISO dropped entirely: the panel is write-only.")
    add("  * GPIO22/23 were listed as reserve. Neither exists on an ESP32-S3.")
    add("")
    add("Changed in v1.2")
    add("-" * 78)
    add("  * Acknowledge button moved GPIO0 -> GPIO47, and the e-Paper clock")
    add("    GPIO3 -> GPIO38. Both were strapping pins: sampled at reset, they change")
    add("    how the chip boots. GPIO0 is the dangerous one - a held button becomes a")
    add("    board that never starts. GPIO3 only selects the JTAG source, but a bus")
    add("    line has no pull at reset and there is no reason to leave a strap floating.")
    add("")
    return "\n".join(line.rstrip() for line in out) + "\n"


# ---------------------------------------------------------------- checks


def check(pins: dict[str, int]) -> list[str]:
    """The same things the firmware's board tests assert, so a bad diagram
    cannot be generated quietly."""
    problems = []

    wanted = [p.key for block in LEFT + RIGHT for p in block.pins]
    seen: dict[int, str] = {}
    for key in wanted:
        if key not in pins:
            problems.append(f"board.rs has no constant for {key}")
            continue
        number = pins[key]
        if number in seen:
            problems.append(f"{key} and {seen[number]} are both GPIO{number}")
        seen[number] = key

    radio_pins = {pins[k] for k in pins if k.startswith("lora.")}
    for key in wanted:
        if key.startswith("epaper.") and pins.get(key) in radio_pins:
            problems.append(f"{key} is GPIO{pins[key]}, which belongs to the SX1262")

    straps = {0, 3, 45, 46}  # boot mode, JTAG source, flash voltage
    for key in wanted:
        if pins.get(key) in straps:
            problems.append(
                f"{key} is GPIO{pins[key]}, which the ESP32-S3 samples at reset"
            )

    for key, number in pins.items():
        if 22 <= number <= 25:
            problems.append(f"{key} is GPIO{number}, which does not exist on an ESP32-S3")
        if 26 <= number <= 32:
            problems.append(f"{key} is GPIO{number}, which is the SiP flash")

    return problems


def main() -> int:
    source = BOARD_RS.read_text()
    pins = parse_board(source)
    radio = parse_radio(source)
    version = "v1.2"

    problems = check(pins)
    if problems:
        print("refusing to draw a diagram that is wrong:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    fit_to_content()

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    (OUT_DIR / "display-node.svg").write_text(build_svg(pins, radio, version))
    (OUT_DIR / "display-node.txt").write_text(build_ascii(pins, radio, version))

    print(f"wrote {OUT_DIR / 'display-node.svg'}")
    print(f"wrote {OUT_DIR / 'display-node.txt'}")
    print(f"{len(pins)} pins parsed, {len(check(pins))} problems")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
