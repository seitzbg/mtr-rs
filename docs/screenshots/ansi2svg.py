#!/usr/bin/env python3
# ansi2svg.py -- turn a `tmux capture-pane -e` dump into a standalone SVG.
#
# Copyright (C) 2026  mtr-rs contributors
#
# This program is free software; you can redistribute it and/or modify it
# under the terms of the GNU General Public License version 2 as published by
# the Free Software Foundation.
#
# This program is distributed in the hope that it will be useful, but WITHOUT
# ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or
# FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public License for
# more details.
#
# SPDX-License-Identifier: GPL-2.0-only
#
# Usage:
#   python3 ansi2svg.py capture.ans out.svg [--title "..."]
#
# Only the Python 3 standard library is used.  The SVG is a grid of cells:
# one <rect> per coloured background run and one <text> per row, with a
# <tspan> per colour run.  Each tspan is pinned to its column with an explicit
# `x` and forced to its exact cell width with `textLength`, so the rendering
# stays aligned even when the reader has none of the preferred fonts.

import argparse
import re
import sys

# --- geometry -------------------------------------------------------------

FONT_STACK = '"DejaVu Sans Mono", "Menlo", "Consolas", monospace'
FONT_SIZE = 14.0
LINE_HEIGHT = FONT_SIZE * 1.2  # 16.8px
CELL_W = 8.4
PAD = 10.0
# Baseline inside the line box: centre the em box, then drop to the baseline.
BASELINE = (LINE_HEIGHT - FONT_SIZE) / 2.0 + FONT_SIZE * 0.8

DEFAULT_FG = "#d4d4d4"
DEFAULT_BG = "#1e1e1e"

# A pleasant dark-theme rendition of the 16 ANSI colours.
PALETTE = [
    "#4b5263",  # 0  black
    "#e06c75",  # 1  red
    "#4ec94e",  # 2  green
    "#e5c07b",  # 3  yellow
    "#61afef",  # 4  blue
    "#c678dd",  # 5  magenta
    "#56b6c2",  # 6  cyan
    "#d4d4d4",  # 7  white
    "#5c6370",  # 8  bright black
    "#ff7b86",  # 9  bright red
    "#7ee787",  # 10 bright green
    "#f0d399",  # 11 bright yellow
    "#82c7ff",  # 12 bright blue
    "#d9a0ea",  # 13 bright magenta
    "#7fd3dd",  # 14 bright cyan
    "#ffffff",  # 15 bright white
]

CUBE = [0x00, 0x5F, 0x87, 0xAF, 0xD7, 0xFF]

SGR_RE = re.compile(r"\x1b\[([0-9;:?]*)([@-~])")
OSC_RE = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
OTHER_ESC_RE = re.compile(r"\x1b[()][0-9A-Za-z]|\x1b[=>]")


def xterm256(n):
    """Map an xterm-256 index onto a hex colour."""
    if n < 16:
        return PALETTE[n]
    if n < 232:
        n -= 16
        r, g, b = CUBE[n // 36], CUBE[(n // 6) % 6], CUBE[n % 6]
        return "#%02x%02x%02x" % (r, g, b)
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


class Style:
    __slots__ = ("fg", "bg", "bold", "dim", "reverse")

    def __init__(self):
        self.fg = None
        self.bg = None
        self.bold = False
        self.dim = False
        self.reverse = False

    def copy(self):
        s = Style()
        s.fg, s.bg = self.fg, self.bg
        s.bold, s.dim, s.reverse = self.bold, self.dim, self.reverse
        return s

    def key(self):
        return (self.fg, self.bg, self.bold, self.dim, self.reverse)

    def resolved(self):
        """(fg, bg or None) after applying reverse video."""
        fg = self.fg or DEFAULT_FG
        bg = self.bg
        if self.reverse:
            fg, bg = bg or DEFAULT_BG, self.fg or DEFAULT_FG
        return fg, bg


def apply_sgr(style, params):
    """Fold one SGR parameter list into `style` (mutating it)."""
    # `ESC[m` is the same as `ESC[0m`.
    codes = [p for p in params.replace(":", ";").split(";")]
    codes = [int(c) if c.isdigit() else 0 for c in codes] or [0]
    i = 0
    while i < len(codes):
        c = codes[i]
        if c == 0:
            style.fg = style.bg = None
            style.bold = style.dim = style.reverse = False
        elif c == 1:
            style.bold = True
        elif c == 2:
            style.dim = True
        elif c == 7:
            style.reverse = True
        elif c in (21, 22):
            style.bold = style.dim = False
        elif c == 27:
            style.reverse = False
        elif 30 <= c <= 37:
            style.fg = PALETTE[c - 30]
        elif 90 <= c <= 97:
            style.fg = PALETTE[c - 90 + 8]
        elif c == 39:
            style.fg = None
        elif 40 <= c <= 47:
            style.bg = PALETTE[c - 40]
        elif 100 <= c <= 107:
            style.bg = PALETTE[c - 100 + 8]
        elif c == 49:
            style.bg = None
        elif c in (38, 48):
            target = "fg" if c == 38 else "bg"
            if i + 1 < len(codes) and codes[i + 1] == 5:
                if i + 2 < len(codes):
                    setattr(style, target, xterm256(codes[i + 2]))
                i += 2
            elif i + 1 < len(codes) and codes[i + 1] == 2:
                if i + 4 < len(codes):
                    r, g, b = codes[i + 2], codes[i + 3], codes[i + 4]
                    setattr(style, target, "#%02x%02x%02x" % (r, g, b))
                i += 4
        i += 1


def parse(text):
    """Return a list of rows; each row is a list of (text, Style) runs."""
    text = OSC_RE.sub("", text)
    text = OTHER_ESC_RE.sub("", text)
    rows = []
    style = Style()
    for raw in text.split("\n"):
        raw = raw.rstrip("\r")
        runs = []
        pos = 0
        for m in SGR_RE.finditer(raw):
            chunk = raw[pos:m.start()]
            if chunk:
                runs.append((chunk, style.copy()))
            if m.group(2) == "m":
                apply_sgr(style, m.group(1))
            pos = m.end()
        chunk = raw[pos:]
        if chunk:
            runs.append((chunk, style.copy()))
        rows.append(runs)
    # Drop trailing blank rows.
    while rows and not "".join(t for t, _ in rows[-1]).strip():
        rows.pop()
    return rows


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def fmt(x):
    return ("%.2f" % x).rstrip("0").rstrip(".")


def render(rows, title=None):
    cols = max((sum(len(t) for t, _ in r) for r in rows), default=0)
    width = cols * CELL_W + 2 * PAD
    height = len(rows) * LINE_HEIGHT + 2 * PAD

    out = ['<?xml version="1.0" encoding="UTF-8"?>']
    out.append(
        '<svg xmlns="http://www.w3.org/2000/svg" width="%s" height="%s" '
        'viewBox="0 0 %s %s" font-family=\'%s\' font-size="%s">'
        % (fmt(width), fmt(height), fmt(width), fmt(height), FONT_STACK, fmt(FONT_SIZE))
    )
    if title:
        out.append("<title>%s</title>" % esc(title))
    out.append(
        '<rect width="100%%" height="100%%" fill="%s" rx="6"/>' % DEFAULT_BG
    )

    # Backgrounds first, so the glyphs sit on top of them.
    bgs = []
    for row, runs in enumerate(rows):
        col = 0
        y = PAD + row * LINE_HEIGHT
        for text, style in runs:
            _, bg = style.resolved()
            n = len(text)
            if bg and bg != DEFAULT_BG and n:
                bgs.append(
                    '<rect x="%s" y="%s" width="%s" height="%s" fill="%s"/>'
                    % (
                        fmt(PAD + col * CELL_W),
                        fmt(y),
                        fmt(n * CELL_W),
                        fmt(LINE_HEIGHT),
                        bg,
                    )
                )
            col += n
    out.extend(bgs)

    for row, runs in enumerate(rows):
        # Trailing blanks paint nothing; dropping them keeps the file small and
        # keeps a copy-paste of the SVG free of padding.
        runs = list(runs)
        while runs and not runs[-1][0].strip():
            runs.pop()
        if runs:
            text, style = runs[-1]
            runs[-1] = (text.rstrip(), style)
        col = 0
        spans = []
        for text, style in runs:
            n = len(text)
            if not n:
                continue
            fg, _ = style.resolved()
            attrs = ['x="%s"' % fmt(PAD + col * CELL_W)]
            attrs.append('textLength="%s"' % fmt(n * CELL_W))
            attrs.append('lengthAdjust="spacing"')
            if fg != DEFAULT_FG:
                attrs.append('fill="%s"' % fg)
            if style.bold:
                attrs.append('font-weight="bold"')
            if style.dim:
                attrs.append('opacity="0.65"')
            spans.append("<tspan %s>%s</tspan>" % (" ".join(attrs), esc(text)))
            col += n
        if not spans:
            continue
        out.append(
            '<text y="%s" fill="%s" xml:space="preserve">%s</text>'
            % (fmt(PAD + row * LINE_HEIGHT + BASELINE), DEFAULT_FG, "".join(spans))
        )

    out.append("</svg>")
    return "\n".join(out) + "\n"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("input", help="tmux capture-pane -e dump")
    ap.add_argument("output", help="SVG to write")
    ap.add_argument("--title", default=None, help="<title> for accessibility")
    args = ap.parse_args(argv)

    with open(args.input, "r", encoding="utf-8", errors="replace") as fh:
        rows = parse(fh.read())
    if not rows:
        sys.exit("%s: nothing to render" % args.input)
    with open(args.output, "w", encoding="utf-8") as fh:
        fh.write(render(rows, args.title))


if __name__ == "__main__":
    main()
