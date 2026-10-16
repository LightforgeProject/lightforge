#!/bin/sh
# Builds the artwork Steam shows for the Lightforge shortcut, from the app
# icon, the wordmark and the launcher's own backdrop colours. Run from this
# directory; needs rsvg-convert and ImageMagick 7. The outputs are committed.
set -eu
ICON=../../../../packaging/icons/hicolor/scalable/apps/pro.tuxcore.Lightforge.svg
WORDMARK=../wordmark.png
BG='#0B0D10'
GLOW='#0A2333'   # theme::ACCENT mixed into theme::BG, as the backdrop's corner

# A dark field with the launcher's accent glow, and a little grain so the
# gradient does not band once it is cut to 8 bits.
field() { # width height out
    magick -size "$1x$2" "radial-gradient:$GLOW-$BG" \
        -attenuate 0.25 +noise Uniform "$3"
}
icon() { # size out
    rsvg-convert -w "$1" -h "$1" "$ICON" -o "$2"
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Library capsule, 600x900: the mark above the name.
field 600 900 "$tmp/tall.png"
icon 300 "$tmp/i300.png"
magick "$tmp/tall.png" \
    "$tmp/i300.png" -gravity center -geometry +0-110 -composite \
    \( "$WORDMARK" -resize 480x \) -gravity center -geometry +0+150 -composite \
    -strip -quality 88 capsule.jpg

# Wide capsule, 920x430: the name alone, which carries the mark already.
field 920 430 "$tmp/wide.png"
magick "$tmp/wide.png" \
    \( "$WORDMARK" -resize 640x \) -gravity center -composite \
    -strip -quality 88 wide.jpg

# Hero, 1920x620: no words, Steam lays the logo over it. The mark, large and
# faint, off to the right where the logo does not go.
field 1920 620 "$tmp/hero.png"
icon 900 "$tmp/i900.png"
magick "$tmp/hero.png" \
    \( "$tmp/i900.png" -channel A -evaluate multiply 0.18 +channel \) \
    -gravity east -geometry -120+0 -composite -strip -quality 88 hero.jpg

# Logo, the wordmark as it is: Steam draws it over the hero at its own size.
magick "$WORDMARK" -strip logo.png

# Icon, square: Steam's list and the Game Mode overlay.
icon 256 icon.png
