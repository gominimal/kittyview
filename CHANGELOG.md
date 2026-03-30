# Changelog

## 0.1.2

- Animated GIF playback via `--animate` flag (kitty animation protocol)
- Animated logo variant with speech bubble (`kittyview --animate logo`)
- Built-in kitten logo now has normal and happy (^_^) expressions

## 0.1.1

- Stdin support: pipe images directly (`cat photo.jpg | kittyview`). Auto-detected when stdin is not a TTY.
- SVG stdin input resolves relative resource paths from the current working directory.

## 0.1.0

Initial release.

- Display images in kitty-compatible terminals (kitty, Ghostty, WezTerm, Konsole, iTerm2)
- Supported raster formats: PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA, QOI, Farbfeld, HDR
- SVG rendering with full text support via resvg (pure Rust)
- Terminal auto-detection via environment variables with `--force` override
- `png` subcommand for format conversion and debugging
- `completions` subcommand for bash, zsh, fish, PowerShell, and elvish
- SVG external resource sandboxing (`--svg-resources none|cwd|tree|any`)
- Oversized SVG downscaling (max 8192x8192) to prevent memory exhaustion
- Crash-safe buffered protocol output
- Built-in kitten logo (`kittyview logo`)
- Cross-platform: Linux (amd64, aarch64), macOS (Intel, Apple Silicon), Windows (amd64, aarch64)
- Pure Rust -- no C dependencies
