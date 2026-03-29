<p align="center">
  <img src="logo.png" width="128" height="128" alt="kittyview logo">
</p>

# kittyview

Display images in your terminal using the [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/).

kittyview renders PNG, JPEG, SVG, and many other image formats directly in your terminal. It auto-detects terminal support and produces clean, chunked output that works with large images.

## Supported terminals

kittyview auto-detects support via environment variables. Confirmed compatible terminals:

- [kitty](https://sw.kovidgoyal.net/kitty/)
- [Ghostty](https://ghostty.org/)
- [WezTerm](https://wezfurlong.org/wezterm/)
- [Konsole](https://konsole.kde.org/)
- [iTerm2](https://iterm2.com/)

Use `--force` if your terminal supports the protocol but isn't detected.

## Install

### Pre-built binaries

Download from [GitHub Releases](../../releases) for Linux (amd64, aarch64), macOS (Intel, Apple Silicon), and Windows (amd64, aarch64). Linux binaries are statically linked.

### From source

```
cargo install --path .
```

## Usage

```
# Display an image
kittyview photo.jpg

# Display an SVG diagram
kittyview architecture.svg

# Display the built-in logo
kittyview logo
```

### Convert to PNG

The `png` subcommand exports any supported format as a PNG file, useful for debugging or format conversion:

```
# Convert SVG to PNG
kittyview png diagram.svg -o diagram.png

# Export the built-in logo
kittyview png --logo -o logo.png

# Pipe to another tool
kittyview png chart.svg | feh -
```

### Shell completions

```
# Bash
kittyview completions bash > ~/.local/share/bash-completion/completions/kittyview

# Zsh
kittyview completions zsh > ~/.local/share/zsh/site-functions

# Fish
kittyview completions fish > ~/.config/fish/completions/kittyview.fish
```

## Supported image formats

| Format   | Extensions                                 |
|----------|--------------------------------------------|
| PNG      | `.png`                                     |
| JPEG     | `.jpg`, `.jpeg`                            |
| GIF      | `.gif`                                     |
| SVG      | `.svg`, `.svgz` (with full text rendering) |
| WebP     | `.webp`                                    |
| BMP      | `.bmp`                                     |
| TIFF     | `.tif`, `.tiff`                            |
| ICO      | `.ico`                                     |
| PNM      | `.ppm`, `.pgm`, `.pbm`                     |
| TGA      | `.tga`                                     |
| QOI      | `.qoi`                                     |
| Farbfeld | `.ff`                                      |
| HDR      | `.hdr`                                     |

SVG files are detected by extension or by content sniffing (`<svg` in the first 1KB).

## SVG resource access

When rendering SVGs, external file references (`<image href="...">`) are blocked by default. Use `--svg-resources` to control this:

| Policy           | Allows                                                     |
|------------------|------------------------------------------------------------|
| `none` (default) | Embedded/inline images only. No file access.               |
| `cwd`            | Files in the current working directory.                    |
| `tree`           | Files in the current working directory and subdirectories. |
| `any`            | Unrestricted file access.                                  |

Data URLs (images embedded directly in the SVG) always work regardless of policy.

```
# Render an SVG that references local images
kittyview --svg-resources tree diagram.svg
```

## Security

- **Terminal detection**: kittyview refuses to emit escape sequences to non-terminal stdout or unsupported terminals unless `--force` is used. This prevents accidental binary output to files or pipes.
- **SVG sandboxing**: External file access from SVGs is blocked by default (`--svg-resources none`).
- **SVG size limits**: Oversized SVGs are automatically downscaled (max 8192x8192) to prevent memory exhaustion.
- **Pure Rust**: No C dependencies. The entire dependency tree compiles from Rust source.
- **Crash safety**: Kitty protocol output is fully buffered before writing to minimize partial escape sequences if the process is interrupted.

## Building from source

Requires Rust 1.85+ (edition 2024).

```
cargo build --release
```

Run tests:

```
cargo test
```

## License

Apache-2.0
