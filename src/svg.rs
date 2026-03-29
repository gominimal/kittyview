// SPDX-License-Identifier: Apache-2.0

use resvg::usvg;
use std::path::Path;

/// Maximum dimension (width or height) for the rendered pixmap.
/// 8192x8192 * 4 bytes = 256MB, which is a reasonable upper bound.
const MAX_DIMENSION: u32 = 8192;

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

/// Render an SVG file to PNG bytes using resvg.
///
/// `svg_path` is used to resolve relative `<image>` references.
/// `policy` controls which external files the SVG is allowed to load.
pub fn render_svg_to_png(
    svg_data: &[u8],
    svg_path: &Path,
    policy: SvgResources,
) -> Result<Vec<u8>, String> {
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

    opt.fontdb_mut().load_system_fonts();
    opt.fontdb_mut().set_sans_serif_family("Liberation Sans");
    opt.fontdb_mut().set_serif_family("Liberation Serif");
    opt.fontdb_mut().set_monospace_family("Liberation Mono");

    let tree =
        usvg::Tree::from_data(svg_data, &opt).map_err(|e| format!("Failed to parse SVG: {e}"))?;

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
}
