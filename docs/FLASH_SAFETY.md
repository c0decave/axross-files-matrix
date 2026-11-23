# Photosensitivity

Back to the [README](../README.md).

The city moves: the digital rain falls, the camera flies, the teleport
warps, search hits breathe slowly. What is done so that the picture does not
flash, and what is not claimed:

## By design

- **No hard cuts of the whole picture.** Flights ease in and out; the
  teleport is at most twice a second, its warp a short streak and a small
  widening of the view, not a white-out.
- **Slow light.** The processor towers follow the load eased over a second;
  the chosen building's highlight breathes at 0.4 Hz, the search pillars at
  0.3 Hz, both well within the rule; the rain's glyphs change one cell at a
  time, every second or two.
- **No saturated red.** No building colour is saturated red (a test checks
  the theme); folders that cannot be read are amber.
- **Static scanlines.** They never move or flicker.
- **Reduce motion** (Settings) stops the rain, the code on the towers and
  the warp, and slows the flights.

## The guard

As axross-amp's, at the output (`src/flash.rs`, `src/render/post.wgsl`):

- The picture as shown is measured on the GPU on a grid of 48 x 27 cells:
  each cell's mean relative luminance and saturated-red value (WCAG's
  definitions), read back two frames later.
- Each cell is followed with hysteresis: a transition is a swing of 0.1 or
  more whose darker end is below 0.8 (or 20 in the red value).
- A *field* is a third of the picture each way (a 10° view when the city
  fills the screen), at a twelfth's stride; it transits when a quarter of
  its cells transit in one direction within 100 ms, alternating with its
  last.
- Where a field has transited four times in the last second, the guard
  lays a veil over it: the picture there is blended towards its own slow
  average, 85 % (95 % from six), and lifted a second after the danger has
  passed.

## Measured

`tests/flash.rs` renders the window frame by frame and holds the frames to
an independent meter (`tests/support/m11.rs`, from axross-amp: at most six
transitions in any second in any field): a boosted flight low over a dense
city, a fast orbit, teleports pressed every frame, the search pillars and
the highlight - and a picture that flashes on purpose at 7.5 Hz, which the
meter must catch without the guard and which the guard must bring within
the rule.

## Not claimed

The tests cover the scenes above at a few window sizes, on Mesa's software
renderer. A monitor's brightness, a very large screen at close range or a
GPU that renders differently are not measured. If you or anyone watching
has epilepsy or feels unwell, stop.
