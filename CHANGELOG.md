# Changelog

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
