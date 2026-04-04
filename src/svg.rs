// SPDX-License-Identifier: Apache-2.0

use resvg::usvg;
use resvg::usvg::fontdb;
use std::path::Path;

/// Default font size used by mermaid-generated SVGs.
const MERMAID_FONT_SIZE: f64 = 16.0;
/// Line height multiplier for multi-line text (matches mermaid's line-height: 1.5).
const LINE_HEIGHT: f64 = 1.5;

/// Maximum dimension (width or height) for the rendered pixmap.
/// 8192x8192 * 4 bytes = 256MB, which is a reasonable upper bound.
const MAX_DIMENSION: u32 = 8192;

/// Check if a font family name exists in the font database.
fn has_font_family(db: &fontdb::Database, name: &str) -> bool {
    db.faces()
        .any(|face| face.families.iter().any(|(f, _)| f == name))
}

/// Set a generic font family mapping to the first available font in the list.
/// If none are found, the mapping is left at fontdb's default.
fn set_family_if_available(
    db: &mut fontdb::Database,
    generic: GenericFamily,
    candidates: &[&str],
) {
    for &name in candidates {
        if has_font_family(db, name) {
            match generic {
                GenericFamily::SansSerif => db.set_sans_serif_family(name),
                GenericFamily::Serif => db.set_serif_family(name),
                GenericFamily::Monospace => db.set_monospace_family(name),
            }
            return;
        }
    }
}

enum GenericFamily {
    SansSerif,
    Serif,
    Monospace,
}

/// Load system fonts and configure generic family mappings with fallbacks.
///
/// Tries common font families in order, falling back gracefully when
/// specific fonts (e.g. Liberation Sans) aren't installed.
pub fn configure_fonts(opts: &mut usvg::Options) {
    opts.fontdb_mut().load_system_fonts();
    set_family_if_available(
        opts.fontdb_mut(),
        GenericFamily::SansSerif,
        &[
            "Liberation Sans",
            "DejaVu Sans",
            "Helvetica Neue",
            "Helvetica",
            "Arial",
        ],
    );
    set_family_if_available(
        opts.fontdb_mut(),
        GenericFamily::Serif,
        &[
            "Liberation Serif",
            "DejaVu Serif",
            "Georgia",
            "Times New Roman",
            "Times",
        ],
    );
    set_family_if_available(
        opts.fontdb_mut(),
        GenericFamily::Monospace,
        &[
            "Liberation Mono",
            "DejaVu Sans Mono",
            "Menlo",
            "Consolas",
            "Courier New",
        ],
    );
}

/// Controls whether SVGs can load external files via `<image href="...">`.
#[derive(Clone, Copy, Debug, Default, PartialEq, clap::ValueEnum)]
pub enum SvgResources {
    /// Only embedded/inline images (data URLs). No file access. Most secure.
    #[default]
    None,
    /// Allow files in the current working directory only (not subdirectories).
    Cwd,
    /// Allow files in the current working directory and its subdirectories.
    Tree,
    /// Unrestricted file access.
    Any,
}

/// Check whether a resolved path is permitted under the given policy.
fn is_path_allowed(path: &Path, policy: SvgResources) -> bool {
    match policy {
        SvgResources::None => false,
        SvgResources::Any => true,
        SvgResources::Cwd | SvgResources::Tree => {
            let canonical = match path.canonicalize() {
                Ok(p) => p,
                Err(_) => return false,
            };
            let cwd = match std::env::current_dir().and_then(|p| p.canonicalize()) {
                Ok(p) => p,
                Err(_) => return false,
            };
            match policy {
                SvgResources::Cwd => canonical.parent().is_some_and(|p| p == cwd),
                SvgResources::Tree => canonical.starts_with(&cwd),
                _ => unreachable!(),
            }
        }
    }
}

/// Build a custom string resolver that enforces the resource access policy.
/// Data URL resolution (embedded images) is always allowed regardless of policy.
fn make_string_resolver(policy: SvgResources) -> usvg::ImageHrefStringResolverFn<'static> {
    if policy == SvgResources::Any {
        return usvg::ImageHrefResolver::default_string_resolver();
    }

    let default = usvg::ImageHrefResolver::default_string_resolver();

    Box::new(move |href: &str, opts: &usvg::Options| {
        if policy == SvgResources::None {
            return Option::None;
        }

        // Resolve the path the same way usvg would, then check policy
        let path = opts.get_abs_path(Path::new(href));
        if !is_path_allowed(&path, policy) {
            return Option::None;
        }

        // Path is within bounds -- delegate to the default loader
        default(href, opts)
    })
}

/// Extract plain-text lines from a foreignObject's HTML content.
///
/// Mermaid wraps label text in `<div><span class="..."><p>text<br/>more</p></span></div>`.
/// We strip all HTML tags (injecting newlines at block boundaries and `<br>`)
/// and split the result into lines.
fn extract_text_lines(html: &str) -> Vec<String> {
    let text = strip_tags(html);
    text.split('\n')
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// Remove XML/HTML tags from a string, returning only text content.
/// Inserts whitespace at structural HTML boundaries (table cells, list items,
/// paragraphs) so that stripped text remains readable.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside_tag = false;
    let mut tag_buf = String::new();
    for ch in s.chars() {
        match ch {
            '<' => {
                inside_tag = true;
                tag_buf.clear();
            }
            '>' => {
                inside_tag = false;
                // Inject separators based on the closing tag name.
                let tag = tag_buf.trim().to_ascii_lowercase();
                let tag_name_raw = tag
                    .strip_prefix('/')
                    .unwrap_or(&tag)
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                let tag_name = tag_name_raw
                    .strip_suffix('/')
                    .unwrap_or(tag_name_raw);
                let is_closing = tag.starts_with('/');
                match tag_name {
                    // <br> / <br/> — always a newline (void element, never has a closing tag)
                    "br" => out.push('\n'),
                    _ if is_closing => match tag_name {
                        // Block-level / row boundaries → newline
                        "tr" | "p" | "div" | "li" | "dt" | "dd" | "h1" | "h2" | "h3" | "h4"
                        | "h5" | "h6" | "blockquote" | "pre" => out.push('\n'),
                        // Cell boundaries → tab
                        "td" | "th" => out.push('\t'),
                        _ => {}
                    },
                    _ => {}
                }
            }
            _ if !inside_tag => out.push(ch),
            _ => tag_buf.push(ch),
        }
    }
    decode_entities(&out)
}

/// Decode HTML/XML character entities and numeric character references.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '&' {
            out.push(ch);
            continue;
        }
        // Collect entity name up to ';' (or give up after 10 chars)
        let mut entity = String::new();
        let mut terminated = false;
        for _ in 0..10 {
            match chars.peek() {
                Some(&';') => {
                    chars.next();
                    terminated = true;
                    break;
                }
                Some(&c) => {
                    entity.push(c);
                    chars.next();
                }
                None => break,
            }
        }
        if !terminated {
            // Not a valid entity reference; emit literally
            out.push('&');
            out.push_str(&entity);
            continue;
        }
        match decode_named_or_numeric(&entity) {
            Some(decoded) => out.push(decoded),
            None => {
                // Unknown entity; emit literally
                out.push('&');
                out.push_str(&entity);
                out.push(';');
            }
        }
    }
    out
}

/// Decode a single named or numeric entity (without the & and ;).
fn decode_named_or_numeric(entity: &str) -> Option<char> {
    // Numeric references: &#NNN; or &#xHHH;
    if let Some(rest) = entity.strip_prefix('#') {
        let code = if let Some(hex) = rest.strip_prefix('x').or_else(|| rest.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            rest.parse::<u32>().ok()?
        };
        return char::from_u32(code);
    }
    // Named entities (common subset)
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{00A0}'),
        "ndash" => Some('\u{2013}'),
        "mdash" => Some('\u{2014}'),
        "lsquo" => Some('\u{2018}'),
        "rsquo" => Some('\u{2019}'),
        "ldquo" => Some('\u{201C}'),
        "rdquo" => Some('\u{201D}'),
        "hellip" => Some('\u{2026}'),
        "bull" => Some('\u{2022}'),
        "copy" => Some('\u{00A9}'),
        "reg" => Some('\u{00AE}'),
        "trade" => Some('\u{2122}'),
        "times" => Some('\u{00D7}'),
        "divide" => Some('\u{00F7}'),
        "larr" => Some('\u{2190}'),
        "rarr" => Some('\u{2192}'),
        "uarr" => Some('\u{2191}'),
        "darr" => Some('\u{2193}'),
        _ => None,
    }
}

/// Information about a single foreignObject to be replaced.
struct ForeignObjectInfo {
    /// Byte range of the entire `<foreignObject ...>...</foreignObject>` in the source.
    start: usize,
    end: usize,
    /// Position and size of the foreignObject region.
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    /// Extracted text lines.
    lines: Vec<String>,
    /// Font size parsed from the SVG or default.
    font_size: f64,
    /// Text fill color (e.g. "#333").
    fill: String,
    /// Font family string (e.g. "trebuchet ms", verdana, arial, sans-serif).
    font_family: String,
}

/// Detect `<foreignObject>` elements in the SVG and replace them with `<text>` elements
/// that resvg can render. Returns the (possibly modified) SVG bytes.
///
/// This is a best-effort conversion targeting mermaid-cli output. If parsing fails or
/// no foreignObject elements are found, the original bytes are returned unchanged.
pub fn convert_foreign_objects(svg_data: &[u8]) -> Vec<u8> {
    let svg_str = match std::str::from_utf8(svg_data) {
        Ok(s) => s,
        Err(_) => return svg_data.to_vec(),
    };

    let doc = match roxmltree::Document::parse(svg_str) {
        Ok(d) => d,
        Err(_) => return svg_data.to_vec(),
    };

    // Detect global defaults from the SVG's embedded styles.
    let font_size = detect_font_size(&doc).unwrap_or(MERMAID_FONT_SIZE);
    let fill = detect_fill(&doc).unwrap_or_else(|| "#000".to_string());
    let font_family = detect_font_family(&doc).unwrap_or_default();

    // Collect all foreignObject elements.
    let mut replacements: Vec<ForeignObjectInfo> = Vec::new();

    for node in doc.descendants() {
        if node.tag_name().name() != "foreignObject" {
            continue;
        }

        // Skip foreignObjects inside a <switch> that already has a fallback sibling.
        // Well-formed SVGs use <switch> with foreignObject + fallback <text>.
        if let Some(parent) = node.parent() {
            if parent.tag_name().name() == "switch"
                && parent.children().any(|c| c.tag_name().name() == "text")
            {
                continue;
            }
        }

        let x: f64 = node
            .attribute("x")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let y: f64 = node
            .attribute("y")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let width: f64 = node
            .attribute("width")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);
        let height: f64 = node
            .attribute("height")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0);

        // Skip empty/zero-sized placeholders
        if width == 0.0 || height == 0.0 {
            continue;
        }

        // Get the raw HTML content inside the foreignObject
        let range = node.range();
        let inner_html = &svg_str[range.start..range.end];

        let lines = extract_text_lines(inner_html);
        if lines.is_empty() {
            continue;
        }

        replacements.push(ForeignObjectInfo {
            start: range.start,
            end: range.end,
            x,
            y,
            width,
            height,
            lines,
            font_size,
            fill: fill.clone(),
            font_family: font_family.clone(),
        });
    }

    if replacements.is_empty() {
        return svg_data.to_vec();
    }

    // Apply replacements in reverse order so byte offsets remain valid.
    let mut result = svg_str.to_string();
    replacements.sort_by(|a, b| b.start.cmp(&a.start));

    for info in &replacements {
        let text_elem = build_text_element(info);
        result.replace_range(info.start..info.end, &text_elem);
    }

    result.into_bytes()
}

/// Build an SVG `<text>` element that approximates the foreignObject's rendered text.
fn build_text_element(info: &ForeignObjectInfo) -> String {
    let cx = info.x + info.width / 2.0;
    let num_lines = info.lines.len();
    let line_step = info.font_size * LINE_HEIGHT;

    // Common attributes shared by single-line and multi-line variants.
    let font_attr = if info.font_family.is_empty() {
        String::new()
    } else {
        format!(" font-family=\"{}\"", xml_escape(&info.font_family))
    };

    if num_lines == 1 {
        let cy = info.y + info.height / 2.0;
        format!(
            "<text x=\"{cx:.1}\" y=\"{cy:.1}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" font-size=\"{fs}\" \
             fill=\"{fill}\"{font}>{text}</text>",
            fs = info.font_size,
            fill = xml_escape(&info.fill),
            font = font_attr,
            text = xml_escape(&info.lines[0]),
        )
    } else {
        // Multi-line: center the block vertically.
        let block_height = (num_lines as f64 - 1.0) * line_step;
        let first_y = info.y + info.height / 2.0 - block_height / 2.0;

        let mut s = format!(
            "<text x=\"{cx:.1}\" text-anchor=\"middle\" \
             dominant-baseline=\"central\" font-size=\"{fs}\" \
             fill=\"{fill}\"{font}>",
            fs = info.font_size,
            fill = xml_escape(&info.fill),
            font = font_attr,
        );
        for (i, line) in info.lines.iter().enumerate() {
            let y = first_y + i as f64 * line_step;
            s.push_str(&format!(
                "<tspan x=\"{cx:.1}\" y=\"{y:.1}\">{text}</tspan>",
                text = xml_escape(line),
            ));
        }
        s.push_str("</text>");
        s
    }
}

/// XML escaping for text content and attribute values.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Try to extract the default font-size from the SVG's embedded styles.
fn detect_font_size(doc: &roxmltree::Document) -> Option<f64> {
    // Look for font-size in the root <svg> style attribute or a <style> element.
    let root = doc.root_element();

    // Check inline style on <svg>
    if let Some(style) = root.attribute("style") {
        if let Some(fs) = parse_font_size_from_css(style) {
            return Some(fs);
        }
    }

    // Check <style> elements
    for node in doc.descendants() {
        if node.tag_name().name() == "style" {
            if let Some(text) = node.text() {
                if let Some(fs) = parse_font_size_from_css(text) {
                    return Some(fs);
                }
            }
        }
    }

    None
}

/// Try to extract the default fill color from the SVG's embedded styles.
fn detect_fill(doc: &roxmltree::Document) -> Option<String> {
    let root = doc.root_element();

    if let Some(style) = root.attribute("style") {
        if let Some(f) = parse_css_property(style, "fill") {
            return Some(f);
        }
    }
    if let Some(fill) = root.attribute("fill") {
        return Some(fill.to_string());
    }
    for node in doc.descendants() {
        if node.tag_name().name() == "style" {
            if let Some(text) = node.text() {
                if let Some(f) = parse_css_property(text, "fill") {
                    return Some(f);
                }
            }
        }
    }
    None
}

/// Try to extract the default font-family from the SVG's embedded styles.
fn detect_font_family(doc: &roxmltree::Document) -> Option<String> {
    let root = doc.root_element();

    if let Some(style) = root.attribute("style") {
        if let Some(f) = parse_css_property(style, "font-family") {
            return Some(f);
        }
    }
    if let Some(ff) = root.attribute("font-family") {
        return Some(ff.to_string());
    }
    for node in doc.descendants() {
        if node.tag_name().name() == "style" {
            if let Some(text) = node.text() {
                if let Some(f) = parse_css_property(text, "font-family") {
                    return Some(f);
                }
            }
        }
    }
    None
}

/// Extract a CSS property value from a CSS fragment.
/// Handles both `property:value;` in inline styles and within rule blocks.
fn parse_css_property(css: &str, property: &str) -> Option<String> {
    let idx = css.find(property)?;
    let rest = &css[idx + property.len()..];
    let rest = rest.trim_start().strip_prefix(':')?;
    let rest = rest.trim_start();
    let end = rest.find([';', '}']).unwrap_or(rest.len());
    let value = rest[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Extract a font-size value in px from a CSS fragment (e.g., "font-size:16px").
fn parse_font_size_from_css(css: &str) -> Option<f64> {
    let idx = css.find("font-size")?;
    let rest = &css[idx + "font-size".len()..];
    let rest = rest.trim_start().strip_prefix(':')?;
    let rest = rest.trim_start();
    // Parse digits and optional decimal
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Render an SVG file to PNG bytes using resvg.
///
/// `svg_path` is used to resolve relative `<image>` references.
/// `policy` controls which external files the SVG is allowed to load.
pub fn render_svg_to_png(
    svg_data: &[u8],
    svg_path: &Path,
    policy: SvgResources,
) -> Result<Vec<u8>, String> {
    // Pre-process: convert foreignObject elements to native <text> so resvg can render them.
    let svg_data = convert_foreign_objects(svg_data);

    let mut opt = usvg::Options::default();

    // Set resources_dir so relative paths in the SVG resolve correctly
    if let Some(parent) = svg_path
        .canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::parent)
    {
        opt.resources_dir = Some(parent.to_path_buf());
    }

    // Apply resource access policy
    opt.image_href_resolver = usvg::ImageHrefResolver {
        resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
        resolve_string: make_string_resolver(policy),
    };

    configure_fonts(&mut opt);

    let tree =
        usvg::Tree::from_data(&svg_data, &opt).map_err(|e| format!("Failed to parse SVG: {e}"))?;

    let size = tree.size().to_int_size();
    let (w, h) = (size.width(), size.height());

    if w == 0 || h == 0 {
        return Err("SVG has zero-sized dimensions".to_string());
    }

    // Fit within MAX_DIMENSION while preserving aspect ratio
    let (w, h, transform) = if w > MAX_DIMENSION || h > MAX_DIMENSION {
        let scale = (MAX_DIMENSION as f32 / w as f32).min(MAX_DIMENSION as f32 / h as f32);
        let sw = (w as f32 * scale).round() as u32;
        let sh = (h as f32 * scale).round() as u32;
        (
            sw,
            sh,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
        )
    } else {
        (w, h, resvg::tiny_skia::Transform::default())
    };

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or("Failed to create render target")?;

    resvg::render(&tree, transform, &mut pixmap.as_mut());

    pixmap
        .encode_png()
        .map_err(|e| format!("Failed to encode SVG render as PNG: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const SIMPLE_SVG: &[u8] = b"\
        <svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
            <rect width='100' height='100' fill='red'/>\
        </svg>";

    #[test]
    fn render_simple_svg() {
        let png =
            render_svg_to_png(SIMPLE_SVG, Path::new("/tmp/test.svg"), SvgResources::None).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), 100);
        assert_eq!(img.height(), 100);
    }

    #[test]
    fn render_svg_with_text() {
        let svg = b"\
            <svg xmlns='http://www.w3.org/2000/svg' width='200' height='50'>\
                <text x='100' y='30' text-anchor='middle' font-size='20'>Hello</text>\
            </svg>";
        let png = render_svg_to_png(svg, Path::new("/tmp/test.svg"), SvgResources::None).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn oversized_svg_is_downscaled() {
        let svg = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' width='20000' height='10000'>\
                <rect width='20000' height='10000' fill='blue'/>\
            </svg>"
        );
        let png = render_svg_to_png(
            svg.as_bytes(),
            Path::new("/tmp/test.svg"),
            SvgResources::None,
        )
        .unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert!(img.width() <= MAX_DIMENSION);
        assert!(img.height() <= MAX_DIMENSION);
        // Aspect ratio preserved: 20000:10000 = 2:1
        assert_eq!(img.width(), MAX_DIMENSION);
        assert_eq!(img.height(), MAX_DIMENSION / 2);
    }

    #[test]
    fn invalid_svg_returns_error() {
        let result = render_svg_to_png(
            b"not xml at all",
            Path::new("/tmp/t.svg"),
            SvgResources::None,
        );
        assert!(result.is_err());
    }

    // --- Resource policy tests ---

    #[test]
    fn policy_none_blocks_all_paths() {
        // Create a real file so canonicalize() can succeed
        let dir = std::env::temp_dir().join("kv_test_policy_none");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.png");
        fs::write(&file, b"fake").unwrap();

        assert!(!is_path_allowed(&file, SvgResources::None));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn policy_any_allows_all_paths() {
        let dir = std::env::temp_dir().join("kv_test_policy_any");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("test.png");
        fs::write(&file, b"fake").unwrap();

        assert!(is_path_allowed(&file, SvgResources::Any));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn policy_cwd_allows_same_dir_only() {
        let cwd = std::env::current_dir().unwrap();
        let file = cwd.join("_kv_test_cwd_policy.tmp");
        fs::write(&file, b"fake").unwrap();

        let subdir = cwd.join("_kv_test_cwd_sub");
        fs::create_dir_all(&subdir).unwrap();
        let subfile = subdir.join("test.tmp");
        fs::write(&subfile, b"fake").unwrap();

        assert!(is_path_allowed(&file, SvgResources::Cwd));
        assert!(!is_path_allowed(&subfile, SvgResources::Cwd));

        fs::remove_file(&file).ok();
        fs::remove_dir_all(&subdir).ok();
    }

    #[test]
    fn policy_tree_allows_subdirs() {
        let cwd = std::env::current_dir().unwrap();
        let subdir = cwd.join("_kv_test_tree_sub");
        fs::create_dir_all(&subdir).unwrap();
        let subfile = subdir.join("test.tmp");
        fs::write(&subfile, b"fake").unwrap();

        assert!(is_path_allowed(&subfile, SvgResources::Tree));

        // File outside CWD should be blocked
        let outside = std::env::temp_dir().join("kv_test_tree_outside.tmp");
        fs::write(&outside, b"fake").unwrap();
        // Only blocked if temp_dir is not under CWD
        if !outside
            .canonicalize()
            .unwrap()
            .starts_with(&cwd.canonicalize().unwrap())
        {
            assert!(!is_path_allowed(&outside, SvgResources::Tree));
        }

        fs::remove_dir_all(&subdir).ok();
        fs::remove_file(&outside).ok();
    }

    #[test]
    fn policy_nonexistent_path_denied() {
        assert!(!is_path_allowed(
            Path::new("/no/such/file.png"),
            SvgResources::Cwd
        ));
        assert!(!is_path_allowed(
            Path::new("/no/such/file.png"),
            SvgResources::Tree
        ));
    }

    // --- foreignObject conversion tests ---

    #[test]
    fn extract_text_single_line() {
        let html = r#"<foreignObject width="100" height="24"><div xmlns="http://www.w3.org/1999/xhtml" style="display: table-cell;"><span class="nodeLabel"><p>Hello World</p></span></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["Hello World"]);
    }

    #[test]
    fn extract_text_multi_line() {
        let html = r#"<foreignObject width="100" height="48"><div xmlns="http://www.w3.org/1999/xhtml"><span class="nodeLabel"><p>forge-api<br />(gRPC gateway)</p></span></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["forge-api", "(gRPC gateway)"]);
    }

    #[test]
    fn extract_text_br_self_closing_no_space() {
        // <br/> without space before / — common in mermaid output
        let html = r#"<foreignObject width="100" height="48"><div><p>line1<br/>line2</p></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["line1", "line2"]);
    }

    #[test]
    fn extract_text_with_entities() {
        let html =
            r#"<foreignObject width="100" height="24"><div><p>A &amp; B</p></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["A & B"]);
    }

    #[test]
    fn extract_text_from_table() {
        let html = r#"<foreignObject width="200" height="60"><div xmlns="http://www.w3.org/1999/xhtml"><table><tr><td>Name</td><td>Value</td></tr><tr><td>CPU</td><td>arm64</td></tr></table></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["Name\tValue", "CPU\tarm64"]);
    }

    #[test]
    fn extract_text_from_list() {
        let html = r#"<foreignObject width="200" height="60"><div xmlns="http://www.w3.org/1999/xhtml"><ul><li>Alpha</li><li>Beta</li><li>Gamma</li></ul></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["Alpha", "Beta", "Gamma"]);
    }

    #[test]
    fn extract_text_from_nested_divs() {
        let html = r#"<foreignObject width="200" height="60"><div><div>First</div><div>Second</div></div></foreignObject>"#;
        let lines = extract_text_lines(html);
        assert_eq!(lines, vec!["First", "Second"]);
    }

    #[test]
    fn convert_foreign_objects_in_svg() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <g transform="translate(50, 50)">
                <foreignObject width="100" height="24">
                    <div xmlns="http://www.w3.org/1999/xhtml">
                        <span class="nodeLabel"><p>Test Label</p></span>
                    </div>
                </foreignObject>
            </g>
        </svg>"#;

        let result = convert_foreign_objects(svg);
        let result_str = std::str::from_utf8(&result).unwrap();

        assert!(
            !result_str.contains("foreignObject"),
            "foreignObject should be replaced"
        );
        assert!(result_str.contains("<text"), "should contain text element");
        assert!(
            result_str.contains("Test Label"),
            "text content should be preserved"
        );
    }

    #[test]
    fn convert_preserves_svg_without_foreign_objects() {
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg' width='100' height='100'>\
            <rect width='100' height='100' fill='red'/></svg>";
        let result = convert_foreign_objects(svg);
        assert_eq!(result, svg.to_vec());
    }

    #[test]
    fn convert_skips_zero_sized_foreign_objects() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <foreignObject width="0" height="0">
                <div xmlns="http://www.w3.org/1999/xhtml"><span class="edgeLabel"></span></div>
            </foreignObject>
        </svg>"#;
        let result = convert_foreign_objects(svg);
        let result_str = std::str::from_utf8(&result).unwrap();
        // Zero-sized foreign objects are kept (no text to replace with)
        assert!(result_str.contains("foreignObject"));
    }

    #[test]
    fn render_mermaid_style_svg_with_foreign_objects() {
        // Simulates a minimal mermaid-like SVG with foreignObject text labels
        let svg = concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="200">"##,
            r##"<style>font-size:16px;</style>"##,
            r##"<rect x="50" y="50" width="200" height="60" fill="#ECECFF" stroke="#9370DB"/>"##,
            r##"<g transform="translate(100, 65)">"##,
            r##"<foreignObject width="100" height="24">"##,
            r##"<div xmlns="http://www.w3.org/1999/xhtml" style="display: table-cell;">"##,
            r##"<span class="nodeLabel"><p>My Node</p></span>"##,
            r##"</div></foreignObject></g></svg>"##,
        );

        let png = render_svg_to_png(
            svg.as_bytes(),
            Path::new("/tmp/test.svg"),
            SvgResources::None,
        )
        .unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        // Verify the rendered image has non-trivial content (not all transparent/white)
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let has_text_pixels = img.pixels().any(|p| p.0[0] < 100 && p.0[3] > 200);
        assert!(
            has_text_pixels,
            "rendered image should contain dark pixels from text"
        );
    }

    #[test]
    fn render_architecture_svg_has_text() {
        let test_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/test/architecture.svg");
        let svg_path = Path::new(test_dir);
        if !svg_path.exists() {
            return; // skip if test files not present
        }
        let svg_data = fs::read(svg_path).unwrap();
        let png = render_svg_to_png(&svg_data, svg_path, SvgResources::None).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();

        // The architecture SVG has text labels -- verify dark pixels exist
        // (text is rendered in #333 = rgb(51,51,51))
        let dark_pixel_count = img
            .pixels()
            .filter(|p| p.0[0] < 80 && p.0[1] < 80 && p.0[2] < 80 && p.0[3] > 200)
            .count();
        assert!(
            dark_pixel_count > 100,
            "architecture.svg should render visible text (got {dark_pixel_count} dark pixels)"
        );
    }

    #[test]
    fn detect_font_size_from_style() {
        assert_eq!(parse_font_size_from_css("font-size:16px;"), Some(16.0));
        assert_eq!(parse_font_size_from_css("font-size: 14px"), Some(14.0));
        assert_eq!(parse_font_size_from_css(r"color:red"), None);
    }

    #[test]
    fn decode_numeric_entities() {
        assert_eq!(decode_entities("&#60;"), "<");
        assert_eq!(decode_entities("&#x3E;"), ">");
        assert_eq!(decode_entities("&#160;"), "\u{00A0}");
        assert_eq!(decode_entities("&#x2192;"), "\u{2192}");
    }

    #[test]
    fn decode_named_entities() {
        assert_eq!(decode_entities("&nbsp;"), "\u{00A0}");
        assert_eq!(decode_entities("&mdash;"), "\u{2014}");
        assert_eq!(decode_entities("&rarr;"), "\u{2192}");
        assert_eq!(decode_entities("&amp;"), "&");
    }

    #[test]
    fn decode_unknown_entity_preserved() {
        assert_eq!(decode_entities("&bogus;"), "&bogus;");
    }

    #[test]
    fn decode_unterminated_ampersand() {
        assert_eq!(decode_entities("AT&T"), "AT&T");
    }

    #[test]
    fn convert_includes_fill_and_font_family() {
        let svg = concat!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">"##,
            r##"<style>#s{fill:#333;font-family:verdana,sans-serif;font-size:14px;}</style>"##,
            r##"<foreignObject width="100" height="24">"##,
            r##"<div xmlns="http://www.w3.org/1999/xhtml"><p>Hi</p></div>"##,
            r##"</foreignObject></svg>"##,
        );
        let result = convert_foreign_objects(svg.as_bytes());
        let result_str = std::str::from_utf8(&result).unwrap();
        assert!(
            result_str.contains(r##"fill="#333""##),
            "should have fill: {result_str}"
        );
        assert!(
            result_str.contains("font-family"),
            "should have font-family: {result_str}"
        );
        assert!(
            result_str.contains("font-size=\"14\""),
            "should detect font-size 14: {result_str}"
        );
    }

    #[test]
    fn convert_respects_xy_on_foreign_object() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <foreignObject x="10" y="20" width="100" height="24">
                <div xmlns="http://www.w3.org/1999/xhtml"><p>Offset</p></div>
            </foreignObject>
        </svg>"#;
        let result = convert_foreign_objects(svg);
        let result_str = std::str::from_utf8(&result).unwrap();
        // x should be 10 + 100/2 = 60, y should be 20 + 24/2 = 32
        assert!(result_str.contains(r#"x="60.0""#), "x offset: {result_str}");
        assert!(result_str.contains(r#"y="32.0""#), "y offset: {result_str}");
    }

    #[test]
    fn convert_skips_foreign_object_in_switch_with_fallback() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <switch>
                <foreignObject width="100" height="24">
                    <div xmlns="http://www.w3.org/1999/xhtml"><p>HTML ver</p></div>
                </foreignObject>
                <text x="50" y="12">Fallback</text>
            </switch>
        </svg>"#;
        let result = convert_foreign_objects(svg);
        let result_str = std::str::from_utf8(&result).unwrap();
        // foreignObject should NOT be replaced since switch has a text fallback
        assert!(
            result_str.contains("foreignObject"),
            "should keep foreignObject in switch: {result_str}"
        );
        assert!(
            result_str.contains("Fallback"),
            "should keep fallback text: {result_str}"
        );
    }

    #[test]
    fn parse_css_property_extracts_values() {
        assert_eq!(
            parse_css_property("fill:#333;color:red", "fill"),
            Some("#333".to_string())
        );
        assert_eq!(
            parse_css_property("font-family:verdana, sans-serif;", "font-family"),
            Some("verdana, sans-serif".to_string())
        );
        assert_eq!(parse_css_property("color:red", "fill"), None);
    }
}
