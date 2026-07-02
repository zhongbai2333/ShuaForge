# Fonts

`MiSans-Regular.otf` is used as the primary bundled UI font for egui on all platforms.
`NotoSansSC-VF.ttf` is kept as a broad CJK fallback font on all platforms.

Source family: Google Noto Sans Simplified Chinese / Noto Sans SC.
License: SIL Open Font License 1.1.

The fonts are embedded to make UI text rendering more consistent across desktop and Android,
instead of depending only on platform-specific font fallback behavior. System fonts are still
registered afterwards as fallback for emoji, symbols, and platform-specific glyph coverage.
