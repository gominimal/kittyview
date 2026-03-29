// SPDX-License-Identifier: Apache-2.0

mod kitty;
mod logo;
mod svg;

use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use std::fs;
use std::io::{self, Cursor, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

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

    /// SVG external resource access policy [default: none]
    #[arg(long, value_enum, global = true, default_value_t)]
    svg_resources: svg::SvgResources,
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
    // Check extension first
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_ascii_lowercase();
        if ext == "svg" || ext == "svgz" {
            return true;
        }
    }
    // Content sniff: look for XML/SVG markers in the first 1KB
    let head = &data[..data.len().min(1024)];
    let head_str = String::from_utf8_lossy(head);
    head_str.contains("<svg") || head_str.contains("<!DOCTYPE svg")
}

/// Load an image file and produce PNG bytes.
/// Routes SVG files through resvg, everything else through the image crate.
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
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode PNG: {e}"))?;
    Ok(buf.into_inner())
}

/// Check whether raw data looks like SVG (content sniffing only, no extension).
fn is_svg_data(data: &[u8]) -> bool {
    let head = &data[..data.len().min(1024)];
    let head_str = String::from_utf8_lossy(head);
    head_str.contains("<svg") || head_str.contains("<!DOCTYPE svg")
}

/// Read stdin and produce PNG bytes.
/// Uses content sniffing for format detection since there is no filename.
/// For SVGs, CWD is used as the base for resolving relative resource paths.
fn load_stdin_as_png(svg_resources: svg::SvgResources) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut data)
        .map_err(|e| format!("Failed to read stdin: {e}"))?;

    if data.is_empty() {
        return Err("No data received on stdin".to_string());
    }

    if is_svg_data(&data) {
        // Use CWD as a synthetic path so resources_dir resolves relative refs from there
        let cwd = std::env::current_dir()
            .unwrap_or_default()
            .join("stdin.svg");
        return svg::render_svg_to_png(&data, &cwd, svg_resources);
    }

    let img = image::load_from_memory(&data)
        .map_err(|e| format!("Failed to decode image from stdin: {e}"))?;
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode PNG: {e}"))?;
    Ok(buf.into_inner())
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

/// Check that it's safe to emit kitty escape sequences.
fn check_terminal(force: bool) -> Result<(), String> {
    if force {
        return Ok(());
    }

    if !io::stdout().is_terminal() {
        return Err(
            "stdout is not a terminal (use --force to emit escape sequences anyway, \
             or use the 'png' subcommand to export as PNG)"
                .to_string(),
        );
    }

    if !kitty::is_supported() {
        return Err(
            "Terminal does not appear to support kitty graphics protocol \
             (use --force to try anyway)"
                .to_string(),
        );
    }

    Ok(())
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Logo) => {
            check_terminal(cli.force)?;
            let png = logo::generate_logo_png();
            let mut stdout = io::stdout().lock();
            kitty::display_png(&png, &mut stdout).map_err(|e| format!("Failed to write: {e}"))?;
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
            let png = match cli.file {
                Some(path) => load_image_as_png(&path, cli.svg_resources)?,
                None if !io::stdin().is_terminal() => load_stdin_as_png(cli.svg_resources)?,
                None => {
                    return Err("No image file specified. Use --help for usage.".to_string());
                }
            };
            check_terminal(cli.force)?;
            let mut stdout = io::stdout().lock();
            kitty::display_png(&png, &mut stdout).map_err(|e| format!("Failed to write: {e}"))?;
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

    // --- is_svg_data (content-only sniffing for stdin) ---

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
