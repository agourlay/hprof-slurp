#!/usr/bin/env sh
# Render the link preview card, site/social.png (1200x630), with ImageMagick.
#
# No browser or external font: the card is composited from shapes and Pango
# text so it renders identically anywhere ImageMagick and DejaVu Sans / Sans
# Mono are present. The palette matches the landing page, and the terminal
# lines are from the run the page shows. Run it after editing the card and
# commit the PNG; the Pages workflow only copies the PNG.
#
#   site/social.sh
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out="$root/site/social.png"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

BG='#0d1117'; LIFT='#161b22'; DEEP='#010409'; TXT='#e6edf3'; DIM='#8b949e'
ACCENT='#f0883e'; AMBER='#d29922'; LINE='rgba(255,255,255,0.10)'
MONO='DejaVu-Sans-Mono'; SANS='DejaVu-Sans'

# Base: a soft top-to-bottom gradient with a faint dot field, the same texture
# the page draws behind its headline.
magick -size 1200x630 gradient:"$LIFT"-"$BG" "$work/base.png"
magick -size 20x20 xc:none -fill 'rgba(255,255,255,0.05)' \
  -draw 'circle 3,3 3,4.3' "$work/dot.png"
magick "$work/base.png" -size 1200x630 tile:"$work/dot.png" \
  -compose over -composite "$work/bg.png"

# The mark, top right: the three bars of the page's logo. Then the terminal
# panel: dark rounded rectangle with a title bar and three dots.
magick "$work/bg.png" \
  -fill "$ACCENT" -draw 'roundrectangle 950,96 1120,124 6,6' \
  -fill 'rgba(230,237,243,0.7)' -draw 'roundrectangle 950,140 1070,168 6,6' \
  -fill 'rgba(230,237,243,0.4)' -draw 'roundrectangle 950,184 1020,212 6,6' \
  -fill "$DEEP" -stroke "$LINE" -strokewidth 1 \
  -draw 'roundrectangle 80,350 1120,566 14,14' \
  -fill "$LIFT" -stroke none \
  -draw 'roundrectangle 81,351 1119,400 14,14' \
  -fill "$DEEP" -draw 'rectangle 81,382 1119,400' \
  -fill '#ff5f56' -draw 'circle 108,376 108,383' \
  -fill '#ffbd2e' -draw 'circle 136,376 136,383' \
  -fill '#27c93f' -draw 'circle 164,376 164,383' \
  "$work/panel.png"

# Text blocks, each rendered transparent then composited at a fixed point.
# Pango takes its font from the markup, not from -font, hence the span.
magick -background none -font "$MONO" -pointsize 72 -fill "$TXT" \
  label:'hprof-slurp' "$work/title.png"
magick -background none -font "$SANS" -pointsize 34 -fill "$DIM" \
  -size 800x caption:'Analyze JVM and Android heap dumps larger than your RAM. One streaming pass, in Rust.' \
  "$work/tag.png"
magick -background none \
  pango:"<span font=\"DejaVu Sans Mono 17\"><span foreground=\"$DIM\">\$</span> <span foreground=\"$TXT\">hprof-slurp heap.hprof</span> <span foreground=\"$AMBER\">--top 8</span></span>" \
  "$work/cmd.png"
magick -background none \
  pango:"<span font=\"DejaVu Sans Mono 17\"><span foreground=\"$DIM\">Found a total of </span><span foreground=\"$ACCENT\" weight=\"bold\">9.27GiB</span><span foreground=\"$DIM\"> of raw shallow heap objects</span></span>" \
  "$work/found.png"
magick -background none \
  pango:"<span font=\"DejaVu Sans Mono 17\"><span foreground=\"$ACCENT\" weight=\"bold\">  2.52GiB</span><span foreground=\"$TXT\">  77411664  byte[]</span>
<span foreground=\"$ACCENT\" weight=\"bold\">  2.31GiB</span><span foreground=\"$TXT\">  77353946  java.lang.String</span></span>" \
  "$work/rows.png"

magick "$work/panel.png" \
  "$work/title.png" -geometry +76+86   -compose over -composite \
  -fill "$ACCENT" -draw 'roundrectangle 80,190 170,196 3,3' \
  "$work/tag.png"   -geometry +80+218  -compose over -composite \
  "$work/cmd.png"   -geometry +108+414 -compose over -composite \
  "$work/found.png" -geometry +108+452 -compose over -composite \
  "$work/rows.png"  -geometry +108+490 -compose over -composite \
  "$out"

printf 'wrote %s (%s)\n' "$out" "$(identify -format '%wx%h' "$out")"
