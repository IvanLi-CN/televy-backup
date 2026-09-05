# TelevyBackup Brand Mark

`televybackup-logo.svg` is the selected Graphite Azure duotone mark. Its first CSS block is the portable color-parameter block: change only `.canvas`, `.disk`, and `.wing` to derive a new colorway while retaining the shared geometry.

- `televybackup-logo-monochrome.svg` uses black for both geometry layers on white.
- `televybackup-logo-dark.svg` has a transparent canvas for dark UI surfaces, a high-contrast blue-gray storage layer, and a light azure transmission layer.
- `televybackup-logo-ui.svg` is the transparent light-appearance Popover asset.
- `televybackup-logo-ui-compact.svg` and `televybackup-logo-dark-compact.svg` use the same paths and colors with a tighter transparent viewBox for the 20pt Popover mark.
- `televybackup-logo-template.svg` is the transparent black template asset used by the system menu bar; AppKit applies the template tint.
- `televybackup-logo-monochrome-design.png` and `televybackup-logo-duotone-design.png` are the approved raster design references, not runtime assets.

`televybackup-logo-compact.svg` is the same geometry with a tighter 1000-point
viewBox for small App Icon sizes. It is used only at 16px and 32px logical sizes;
the canonical source remains the source of truth for 128px and larger outputs.

The macOS App Icon source groups live under `macos/layers/{default,dark,mono}`.
They are vector-only appearance sources. `Assets.xcassets/AppIcon.appiconset` is
generated from those sources with Default, Dark, and Tinted/Mono appearance entries;
macOS compiles it to `Assets.car` at build time. The legacy ICNS remains in the
bundle as an explicit fallback.

The macOS application icon is derived from the white-background light SVG into
`macos/TelevyBackup.iconset/`, `macos/Assets.xcassets/AppIcon.appiconset/`, and
`macos/TelevyBackupApp/Resources/TelevyBackup.icns`. Deterministic mask previews
are stored under `macos/previews/` for 48px, 128px, and 512px review at rounded,
squircle, and circle masks.
Run `scripts/macos/generate-brand-variants.sh` and
`scripts/macos/generate-app-icon-assets.sh` after changing the canonical geometry,
then run `scripts/macos/generate-app-icon-previews.sh` and
`scripts/macos/verify-app-icon-assets.sh` to check dimensions, alpha, the asset
catalog compilation, and the ICNS round trip.

Each SVG contains a background rectangle and two vector paths: the storage-drive silhouette and the abstract transmission wing. No SVG contains embedded or linked raster data.
