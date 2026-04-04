// SPDX-License-Identifier: Apache-2.0

mod kitty;
mod logo;
mod svg;
mod terminal;

use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use std::fs;
use std::io::{self, Cursor, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use terminal::Mux;

/// Parse the `--passthrough` flag value into a mux stack.
///
/// Accepts: `auto`, `off`, or a comma-separated list of `tmux`/`screen`
/// (innermost first). Examples: `tmux`, `tmux,tmux`, `tmux,screen`.
fn parse_passthrough(s: &str) -> Result<Option<Vec<Mux>>, String> {
    let s = s.trim();
    match s {
        "auto" => Ok(None),
        "off" => Ok(Some(vec![])),
        _ => {
            let mut stack = Vec::new();
            for part in s.split(',') {
                let part = part.trim();
                match part {
                    "tmux" => stack.push(Mux::Tmux(None)),
                    "screen" => stack.push(Mux::Screen(None)),
                    _ => {
                        return Err(format!(
                            "invalid passthrough value '{part}': expected 'auto', 'off', \
                             or comma-separated list of 'tmux'/'screen' (e.g. 'tmux,tmux')"
                        ));
                    }
                }
            }
            if stack.is_empty() {
                return Err("empty passthrough value".to_string());
            }
            Ok(Some(stack))
        }
    }
}

#[derive(Parser)]
#[command(
    name = "kittyview",
    version,
    about = "Display images in kitty-compatible terminals"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Image file to display
    #[arg(value_hint = ValueHint::FilePath)]
    file: Option<PathBuf>,

    /// Force output even if terminal support is not detected
    #[arg(long, global = true)]
    force: bool,

    /// Play animated images (e.g. GIF) instead of showing first frame
    #[arg(long, global = true)]
    animate: bool,

    /// SVG external resource access policy
    #[arg(long, value_enum, global = true, default_value_t)]
    svg_resources: svg::SvgResources,

    /// Multiplexer passthrough: auto, off, or comma-separated tmux/screen layers
    /// (e.g. tmux, tmux,tmux, tmux,screen)
    #[arg(long, global = true, default_value = "auto")]
    passthrough: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Display the built-in Rust cat logo
    Logo,
    /// Export an image (or the logo) as PNG
    Png {
        /// Input image file (omit to export the logo)
        #[arg(value_hint = ValueHint::FilePath)]
        input: Option<PathBuf>,

        /// Output file (default: stdout)
        #[arg(short, long, value_hint = ValueHint::FilePath)]
        output: Option<PathBuf>,

        /// Export the built-in logo instead of an input file
        #[arg(long)]
        logo: bool,
    },
    /// Generate shell completions
    Completions {
        /// Shell type
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

/// Detect whether file data looks like SVG (by extension or content sniffing).
fn is_svg(path: &std::path::Path, data: &[u8]) -> bool {
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_ascii_lowercase();
        if ext == "svg" || ext == "svgz" {
            return true;
        }
    }
    is_svg_data(data)
}

/// Check whether raw data looks like SVG (content sniffing only, no extension).
fn is_svg_data(data: &[u8]) -> bool {
    let head = &data[..data.len().min(1024)];
    let head_str = String::from_utf8_lossy(head);
    head_str.contains("<svg") || head_str.contains("<!DOCTYPE svg")
}

/// Encode a DynamicImage as PNG bytes.
fn encode_png(img: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode PNG: {e}"))?;
    Ok(buf.into_inner())
}

/// Load an image file and produce PNG bytes (single frame).
fn load_image_as_png(
    path: &std::path::Path,
    svg_resources: svg::SvgResources,
) -> Result<Vec<u8>, String> {
    let data = fs::read(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

    if is_svg(path, &data) {
        return svg::render_svg_to_png(&data, path, svg_resources);
    }

    let img = image::load_from_memory(&data)
        .map_err(|e| format!("Failed to decode {}: {e}", path.display()))?;
    encode_png(&img)
}

/// Read stdin bytes, returning an error if empty.
fn read_stdin() -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut data)
        .map_err(|e| format!("Failed to read stdin: {e}"))?;
    if data.is_empty() {
        return Err("No data received on stdin".to_string());
    }
    Ok(data)
}

/// Synthetic SVG path for stdin (CWD-based resource resolution).
fn stdin_svg_path() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_default()
        .join("stdin.svg")
}

/// Read stdin and produce PNG bytes (single frame).
fn load_stdin_as_png(svg_resources: svg::SvgResources) -> Result<Vec<u8>, String> {
    let data = read_stdin()?;

    if is_svg_data(&data) {
        let path = stdin_svg_path();
        return svg::render_svg_to_png(&data, &path, svg_resources);
    }

    let img = image::load_from_memory(&data)
        .map_err(|e| format!("Failed to decode image from stdin: {e}"))?;
    encode_png(&img)
}

/// Decode GIF frames as (PNG bytes, delay_ms) pairs.
fn decode_gif_frames(data: &[u8]) -> Result<Vec<(Vec<u8>, u32)>, String> {
    use image::AnimationDecoder;
    use image::codecs::gif::GifDecoder;

    let decoder =
        GifDecoder::new(Cursor::new(data)).map_err(|e| format!("Failed to decode GIF: {e}"))?;
    let frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|e| format!("Failed to read GIF frames: {e}"))?;

    let mut result = Vec::with_capacity(frames.len());
    for frame in frames {
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay_ms = if denom == 0 { 100 } else { numer / denom };
        let delay_ms = delay_ms.max(20); // floor at 20ms to prevent 0-delay spam

        let img = image::DynamicImage::ImageRgba8(frame.into_buffer());
        result.push((encode_png(&img)?, delay_ms));
    }

    Ok(result)
}

/// Load an image file as animation frames. Falls back to single frame for non-animated formats.
fn load_animated_image(
    path: &std::path::Path,
    svg_resources: svg::SvgResources,
) -> Result<Vec<(Vec<u8>, u32)>, String> {
    let data = fs::read(path).map_err(|e| format!("Failed to read {}: {e}", path.display()))?;

    if is_svg(path, &data) {
        let png = svg::render_svg_to_png(&data, path, svg_resources)?;
        return Ok(vec![(png, 0)]);
    }

    if let Ok(image::ImageFormat::Gif) = image::guess_format(&data) {
        let frames = decode_gif_frames(&data)?;
        if frames.len() > 1 {
            return Ok(frames);
        }
    }

    let img = image::load_from_memory(&data)
        .map_err(|e| format!("Failed to decode {}: {e}", path.display()))?;
    Ok(vec![(encode_png(&img)?, 0)])
}

/// Load stdin as animation frames.
fn load_animated_stdin(svg_resources: svg::SvgResources) -> Result<Vec<(Vec<u8>, u32)>, String> {
    let data = read_stdin()?;

    if is_svg_data(&data) {
        let path = stdin_svg_path();
        let png = svg::render_svg_to_png(&data, &path, svg_resources)?;
        return Ok(vec![(png, 0)]);
    }

    if let Ok(image::ImageFormat::Gif) = image::guess_format(&data) {
        let frames = decode_gif_frames(&data)?;
        if frames.len() > 1 {
            return Ok(frames);
        }
    }

    let img = image::load_from_memory(&data)
        .map_err(|e| format!("Failed to decode image from stdin: {e}"))?;
    Ok(vec![(encode_png(&img)?, 0)])
}

/// Write bytes to a file or stdout.
fn write_output(data: &[u8], path: Option<&std::path::Path>) -> Result<(), String> {
    match path {
        Some(p) => fs::write(p, data).map_err(|e| format!("Failed to write {}: {e}", p.display())),
        None => io::stdout()
            .lock()
            .write_all(data)
            .map_err(|e| format!("Failed to write stdout: {e}")),
    }
}

/// Detect the terminal and multiplexer, check kitty graphics support.
///
/// Returns the detected mux stack (for passthrough wrapping) or an error if the
/// terminal doesn't support kitty graphics and `--force` is not set.
fn check_terminal(
    force: bool,
    passthrough: &str,
) -> Result<Vec<Mux>, String> {
    if !force && !io::stdout().is_terminal() {
        return Err(
            "stdout is not a terminal (use --force to emit escape sequences anyway, \
             or use the 'png' subcommand to export as PNG)"
                .to_string(),
        );
    }

    let parsed = parse_passthrough(passthrough)?;

    // When force is set without an explicit passthrough mode, skip detection
    // to avoid the query timeout delay.
    if force && parsed.is_none() {
        return Ok(vec![]);
    }

    // Resolve the mux stack.
    let mux_stack = match parsed {
        Some(stack) => stack,
        None => {
            let info = terminal::detect();

            if !force && !info.supports_kitty_graphics() {
                let mut msg = format!(
                    "Terminal ({}) does not appear to support kitty graphics protocol",
                    info.terminal
                );
                if !info.mux_stack.is_empty() {
                    let mux_desc: Vec<String> =
                        info.mux_stack.iter().map(|m| m.to_string()).collect();
                    msg.push_str(&format!(
                        " (detected multiplexer{}: {})",
                        if info.mux_stack.len() > 1 { "s" } else { "" },
                        mux_desc.join(" > "),
                    ));
                    if info.mux_stack.iter().any(|m| matches!(m, Mux::Tmux(_))) {
                        msg.push_str(
                            "\nHint: ensure the outer terminal supports kitty graphics \
                             and add `set -g allow-passthrough on` to your tmux.conf",
                        );
                    }
                }
                msg.push_str(" (use --force to try anyway)");
                return Err(msg);
            }

            info.mux_stack
        }
    };

    Ok(mux_stack)
}

/// Display frames to stdout -- animation if multi-frame, static if single.
fn display_frames(
    frames: &[(Vec<u8>, u32)],
    out: &mut impl Write,
    mux_stack: &[Mux],
) -> Result<(), String> {
    if frames.len() > 1 {
        kitty::display_animation(frames, out, mux_stack)
            .map_err(|e| format!("Failed to write: {e}"))
    } else if let Some((png, _)) = frames.first() {
        kitty::display_png(png, out, mux_stack).map_err(|e| format!("Failed to write: {e}"))
    } else {
        Ok(())
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Logo) => {
            let mux_stack = check_terminal(cli.force, &cli.passthrough)?;
            let mut stdout = io::stdout().lock();
            if cli.animate {
                let frames = logo::generate_animated_logo();
                display_frames(&frames, &mut stdout, &mux_stack)?;
            } else {
                let png = logo::generate_logo_png();
                kitty::display_png(&png, &mut stdout, &mux_stack)
                    .map_err(|e| format!("Failed to write: {e}"))?;
            }
            writeln!(stdout).map_err(|e| format!("Failed to write: {e}"))?;
            Ok(())
        }
        Some(Commands::Png {
            input,
            output,
            logo,
        }) => {
            let png = match (input, logo) {
                (_, true) => logo::generate_logo_png(),
                (Some(path), false) => load_image_as_png(&path, cli.svg_resources)?,
                (None, false) if !io::stdin().is_terminal() => {
                    load_stdin_as_png(cli.svg_resources)?
                }
                (None, false) => {
                    return Err("Provide an input file or use --logo".to_string());
                }
            };
            write_output(&png, output.as_deref())
        }
        Some(Commands::Completions { shell }) => {
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "kittyview", &mut io::stdout());
            Ok(())
        }
        None => {
            let mux_stack = check_terminal(cli.force, &cli.passthrough)?;
            let mut stdout = io::stdout().lock();

            if cli.animate {
                let frames = match cli.file {
                    Some(path) => load_animated_image(&path, cli.svg_resources)?,
                    None if !io::stdin().is_terminal() => load_animated_stdin(cli.svg_resources)?,
                    None => {
                        return Err("No image file specified. Use --help for usage.".to_string());
                    }
                };
                display_frames(&frames, &mut stdout, &mux_stack)?;
            } else {
                let png = match cli.file {
                    Some(path) => load_image_as_png(&path, cli.svg_resources)?,
                    None if !io::stdin().is_terminal() => load_stdin_as_png(cli.svg_resources)?,
                    None => {
                        return Err("No image file specified. Use --help for usage.".to_string());
                    }
                };
                kitty::display_png(&png, &mut stdout, &mux_stack)
                    .map_err(|e| format!("Failed to write: {e}"))?;
            }

            writeln!(stdout).map_err(|e| format!("Failed to write: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn svg_detected_by_extension() {
        assert!(is_svg(Path::new("diagram.svg"), b"anything"));
        assert!(is_svg(Path::new("diagram.SVG"), b"anything"));
        assert!(is_svg(Path::new("file.svgz"), b"anything"));
    }

    #[test]
    fn svg_detected_by_content_sniffing() {
        assert!(is_svg(
            Path::new("no_ext"),
            b"<?xml version='1.0'?><svg xmlns='...'>"
        ));
        assert!(is_svg(Path::new("file.xml"), b"<!DOCTYPE svg"));
    }

    #[test]
    fn non_svg_not_detected() {
        assert!(!is_svg(Path::new("photo.png"), b"\x89PNG\r\n\x1a\n"));
        assert!(!is_svg(Path::new("image.jpg"), b"\xff\xd8\xff"));
        assert!(!is_svg(Path::new("file.txt"), b"just some text"));
    }

    #[test]
    fn svg_data_detected_by_content() {
        assert!(is_svg_data(b"<svg xmlns='http://www.w3.org/2000/svg'>"));
        assert!(is_svg_data(b"<?xml version='1.0'?><svg>"));
        assert!(is_svg_data(b"<!DOCTYPE svg PUBLIC"));
    }

    #[test]
    fn non_svg_data_not_detected() {
        assert!(!is_svg_data(b"\x89PNG\r\n\x1a\n"));
        assert!(!is_svg_data(b"\xff\xd8\xff"));
        assert!(!is_svg_data(b"just some text"));
        assert!(!is_svg_data(b"<html><body>not svg</body></html>"));
        assert!(!is_svg_data(b""));
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("Error: {msg}");
            ExitCode::FAILURE
        }
    }
}
