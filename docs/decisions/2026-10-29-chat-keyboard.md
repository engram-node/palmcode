# Chat keyboard viewport and rounded inputs

Use `adjustResize` on MainActivity rather than Android's implicit keyboard
adjustment choice. With the edge-to-edge window, Compose consumes system bars,
display cutouts and IME insets on the chat root; the weighted transcript shrinks
and the composer remains at the bottom. Follow the last transcript item as IME
height changes so opening the keyboard does not hide the most recent turn.

Reject panning: it moved the composer near the top and pushed the header out of
view on the physical Android 16 device. Do not hide the keyboard on Send to
work around incorrect layout. Explicit theme shapes give outlined inputs
12 dp corners, with a 20 dp composer; buttons keep Material's pill defaults.
