// SPDX-License-Identifier: Apache-2.0

mod geometry;
mod kitty;
mod logo;
mod placeholder;
mod svg;
mod terminal;

use clap::{CommandFactory, Parser, Subcommand, ValueEnum, ValueHint};
use kitty::Placement;
use std::fs;
use std::io::{self, Cursor, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use terminal::{Mux, Terminal};

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

    /// How images are anchored to the screen: auto, unicode (placeholder cells
    /// that scroll with the text), or direct (terminal-positioned)
    #[arg(long, value_enum, global = true, default_value_t)]
    placement: PlacementMode,
}

/// User-facing choice of image anchoring.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
enum PlacementMode {
    /// Unicode placeholders wherever the terminal understands them.
    #[default]
    Auto,
    /// Always anchor to Unicode placeholder cells.
    Unicode,
    /// Always let the terminal position the image itself.
    Direct,
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
        let delay_ms = numer.checked_div(denom).unwrap_or(100);
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

/// What we are drawing into: the multiplexer layers to wrap for, and the
/// terminal underneath them.
struct Target {
    mux_stack: Vec<Mux>,
    terminal: Terminal,
}

impl Target {
    /// Whether images should be anchored to Unicode placeholder cells.
    ///
    /// Under a multiplexer this is the only anchoring that behaves: a directly
    /// placed image is positioned by the terminal, which knows nothing of the
    /// multiplexer's panes or scrollback, so it stays pinned to the screen
    /// while the text beneath it scrolls away. Placeholder cells are ordinary
    /// text, so the multiplexer scrolls and clips them like anything else.
    fn use_placeholders(&self, mode: PlacementMode) -> bool {
        match mode {
            PlacementMode::Unicode => true,
            PlacementMode::Direct => false,
            PlacementMode::Auto => {
                let info = terminal::TerminalInfo {
                    mux_stack: self.mux_stack.clone(),
                    terminal: self.terminal.clone(),
                };
                info.supports_unicode_placeholders()
            }
        }
    }
}

/// Detect the terminal and multiplexer, check kitty graphics support.
///
/// Returns the detected target (for passthrough wrapping and placement
/// selection) or an error if the terminal doesn't support kitty graphics and
/// `--force` is not set.
fn check_terminal(force: bool, passthrough: &str) -> Result<Target, String> {
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
        return Ok(Target {
            mux_stack: vec![],
            terminal: Terminal::Unknown,
        });
    }

    // Resolve the mux stack.
    let (mux_stack, terminal) = match parsed {
        // An explicit stack skips detection, so the terminal stays unidentified.
        Some(stack) => (stack, Terminal::Unknown),
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

            (info.mux_stack, info.terminal)
        }
    };

    warn_if_tmux_passthrough_disabled(&mux_stack);

    Ok(Target {
        mux_stack,
        terminal,
    })
}

/// Warn when tmux is configured to drop the escape sequences we are about to
/// send, which would otherwise look like kittyview silently doing nothing.
fn warn_if_tmux_passthrough_disabled(mux_stack: &[Mux]) {
    if !mux_stack.iter().any(|m| matches!(m, Mux::Tmux(_))) || std::env::var_os("TMUX").is_none() {
        return;
    }
    let Ok(out) = Command::new("tmux")
        .args(["show", "-gv", "allow-passthrough"])
        .output()
    else {
        return;
    };
    if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "off" {
        eprintln!(
            "Warning: tmux `allow-passthrough` is off, so the image data will not reach \
             the terminal.\nAdd `set -g allow-passthrough on` to your tmux.conf."
        );
    }
}

/// Decide how to anchor an image of `png`'s dimensions.
///
/// Falls back to direct placement when the image rectangle cannot be worked
/// out, which is better than emitting a placeholder grid of the wrong shape.
fn placement_for(png: &[u8], mux_stack: &[Mux], use_placeholders: bool) -> Placement {
    if !use_placeholders {
        return Placement::Direct;
    }
    let Some((px_w, px_h)) = geometry::png_dimensions(png) else {
        return Placement::Direct;
    };

    let geom = geometry::detect(mux_stack);
    let max_cells = u16::try_from(placeholder::MAX_INDEX + 1).unwrap_or(u16::MAX);
    let (cols, rows) = geometry::image_cells(px_w, px_h, &geom, max_cells);
    if cols == 0 || rows == 0 {
        return Placement::Direct;
    }

    Placement::Virtual {
        image_id: kitty::pick_image_id(mux_stack),
        cols,
        rows,
    }
}

/// Display frames to stdout -- animation if multi-frame, static if single.
fn display_frames(
    frames: &[(Vec<u8>, u32)],
    out: &mut impl Write,
    mux_stack: &[Mux],
    placement: Placement,
) -> Result<(), String> {
    if frames.len() > 1 {
        kitty::display_animation(frames, out, mux_stack, placement)
            .map_err(|e| format!("Failed to write: {e}"))
    } else if let Some((png, _)) = frames.first() {
        kitty::display_png(png, out, mux_stack, placement)
            .map_err(|e| format!("Failed to write: {e}"))
    } else {
        Ok(())
    }
}

/// Display frames, sizing the placement from the first frame.
fn display_sized_frames(
    frames: &[(Vec<u8>, u32)],
    out: &mut impl Write,
    target: &Target,
    mode: PlacementMode,
) -> Result<(), String> {
    let use_placeholders = target.use_placeholders(mode);
    let placement = match frames.first() {
        Some((png, _)) => placement_for(png, &target.mux_stack, use_placeholders),
        None => return Ok(()),
    };
    display_frames(frames, out, &target.mux_stack, placement)
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Logo) => {
            let target = check_terminal(cli.force, &cli.passthrough)?;
            let frames = if cli.animate {
                logo::generate_animated_logo()
            } else {
                vec![(logo::generate_logo_png(), 0)]
            };
            let mut stdout = io::stdout().lock();
            display_sized_frames(&frames, &mut stdout, &target, cli.placement)?;
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
            let target = check_terminal(cli.force, &cli.passthrough)?;

            let frames = if cli.animate {
                match cli.file {
                    Some(path) => load_animated_image(&path, cli.svg_resources)?,
                    None if !io::stdin().is_terminal() => load_animated_stdin(cli.svg_resources)?,
                    None => {
                        return Err("No image file specified. Use --help for usage.".to_string());
                    }
                }
            } else {
                let png = match cli.file {
                    Some(path) => load_image_as_png(&path, cli.svg_resources)?,
                    None if !io::stdin().is_terminal() => load_stdin_as_png(cli.svg_resources)?,
                    None => {
                        return Err("No image file specified. Use --help for usage.".to_string());
                    }
                };
                vec![(png, 0)]
            };

            let mut stdout = io::stdout().lock();
            display_sized_frames(&frames, &mut stdout, &target, cli.placement)?;
            writeln!(stdout).map_err(|e| format!("Failed to write: {e}"))?;
            Ok(())
        }
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
