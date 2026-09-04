# TelevyBackup Brand Mark

`televybackup-logo.svg` is the selected Graphite Azure duotone mark. Its first CSS block is the portable color-parameter block: change only `.canvas`, `.disk`, and `.wing` to derive a new colorway while retaining the shared geometry.

- `televybackup-logo-monochrome.svg` uses black for both geometry layers on white.
- `televybackup-logo-dark.svg` has a transparent canvas for dark UI surfaces, a muted blue-gray storage layer, and a brighter transmission layer.
- `televybackup-logo-ui.svg` is the transparent light-appearance Popover asset.
- `televybackup-logo-template.svg` is the transparent black template asset used by the system menu bar; AppKit applies the template tint.
- `televybackup-logo-monochrome-design.png` and `televybackup-logo-duotone-design.png` are the approved raster design references, not runtime assets.

The macOS application icon is derived from the white-background light SVG into
`macos/TelevyBackup.iconset/` and `macos/TelevyBackupApp/Resources/TelevyBackup.icns`.
Run `scripts/macos/generate-brand-variants.sh` and
`scripts/macos/generate-app-icon-assets.sh` after changing the canonical geometry,
then run `scripts/macos/verify-app-icon-assets.sh` to check dimensions, alpha, and
the ICNS round trip.

Each SVG contains a background rectangle and two vector paths: the storage-drive silhouette and the abstract transmission wing. No SVG contains embedded or linked raster data.
