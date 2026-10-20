# App artwork

The current logo is the owner-supplied `controlly-logo.png` at the repository root.
It is a 512 × 512 transparent PNG, cropped to its visible mark and compressed for
distribution. The untouched, extensionless original is kept locally and ignored.

Run `scripts/make-icon.ps1` after replacing it. This generates the window/titlebar
PNG, the multi-resolution application icon, and the three small tray status icons.
The build also regenerates them when the source logo changes. Only these small
derivatives are included in the release ZIP.

Optimize new PNG artwork before committing it, for example with
`oxipng -o 4 --nx --strip tEXt,zTXt,iTXt,tIME,eXIf --zopfli <file.png>`.
Keep dimensions, decoded pixels, transparency and color-profile metadata intact.
Icon generation preserves the compressed app PNG when its pixels are unchanged.
The optimizer is a development tool, not a build or runtime dependency.
