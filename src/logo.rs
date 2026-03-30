// SPDX-License-Identifier: Apache-2.0

use image::RgbaImage;

const GRID: u32 = 16;
const SCALE: u32 = 8;

// 0=transparent, 1=lavender purple, 2=dark indigo, 3=white, 4=pink
const PALETTE: [[u8; 4]; 5] = [
    [0, 0, 0, 0],         // 0: transparent
    [155, 143, 232, 255], // 1: lavender purple (#9B8FE8)
    [45, 27, 105, 255],   // 2: dark indigo (#2D1B69)
    [255, 255, 255, 255], // 3: white (eye highlight)
    [255, 145, 176, 255], // 4: warm pink (#FF91B0)
];

const _: () = assert!(PALETTE.len() >= 5);

// 16x16 pixel-art kitten -- compiler checks both dimensions
#[rustfmt::skip]
const LOGO: [[u8; 16]; 16] = [
//   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5
    [0,0,0,2,0,0,0,0,0,0,0,0,2,0,0,0], //  0  ear tips
    [0,0,2,4,2,0,0,0,0,0,0,2,4,2,0,0], //  1  ears (pink inner)
    [0,2,1,4,2,0,0,0,0,0,0,2,4,1,2,0], //  2  ears wider (pink inner)
    [0,2,1,1,2,2,2,2,2,2,2,2,1,1,2,0], //  3  ears -> head
    [0,2,1,1,1,1,1,1,1,1,1,1,1,1,2,0], //  4  forehead
    [0,2,1,2,3,2,1,1,1,1,2,3,2,1,2,0], //  5  eyes  (D W D / D W D)
    [0,2,1,2,2,2,1,1,1,1,2,2,2,1,2,0], //  6  eyes  (D D D / D D D)
    [0,2,1,4,4,1,1,1,1,1,1,4,4,1,2,0], //  7  blush (2px per cheek)
    [0,2,1,1,1,1,1,4,4,1,1,1,1,1,2,0], //  8  nose
    [0,2,1,1,1,1,2,1,1,2,1,1,1,1,2,0], //  9  mouth (w top)
    [0,2,1,1,1,1,1,2,2,1,1,1,1,1,2,0], // 10  mouth (w bottom)
    [0,0,2,1,1,1,1,1,1,1,1,1,1,2,0,0], // 11  chin narrows
    [0,0,0,2,1,1,1,1,1,1,1,1,2,0,0,0], // 12  lower chin
    [0,0,0,0,2,2,2,2,2,2,2,2,0,0,0,0], // 13  bottom edge
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], // 14  blank
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], // 15  blank
];

// Happy variant: ^_^ closed eyes (rows 5-6 differ)
#[rustfmt::skip]
const LOGO_HAPPY: [[u8; 16]; 16] = [
//   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5
    [0,0,0,2,0,0,0,0,0,0,0,0,2,0,0,0], //  0
    [0,0,2,4,2,0,0,0,0,0,0,2,4,2,0,0], //  1
    [0,2,1,4,2,0,0,0,0,0,0,2,4,1,2,0], //  2
    [0,2,1,1,2,2,2,2,2,2,2,2,1,1,2,0], //  3
    [0,2,1,1,1,1,1,1,1,1,1,1,1,1,2,0], //  4
    [0,2,1,1,2,1,1,1,1,1,1,2,1,1,2,0], //  5  happy eyes: _ (line at bottom)
    [0,2,1,2,1,2,1,1,1,1,2,1,2,1,2,0], //  6  happy eyes: V shape below
    [0,2,1,4,4,1,1,1,1,1,1,4,4,1,2,0], //  7
    [0,2,1,1,1,1,1,4,4,1,1,1,1,1,2,0], //  8
    [0,2,1,1,1,1,2,1,1,2,1,1,1,1,2,0], //  9
    [0,2,1,1,1,1,1,2,2,1,1,1,1,1,2,0], // 10
    [0,0,2,1,1,1,1,1,1,1,1,1,1,2,0,0], // 11
    [0,0,0,2,1,1,1,1,1,1,1,1,2,0,0,0], // 12
    [0,0,0,0,2,2,2,2,2,2,2,2,0,0,0,0], // 13
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], // 14
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], // 15
];

/// Draw a scaled pixel block onto an image.
fn put_scaled(img: &mut RgbaImage, x: u32, y: u32, scale: u32, color: [u8; 4]) {
    for dy in 0..scale {
        for dx in 0..scale {
            img.put_pixel(x * scale + dx, y * scale + dy, image::Rgba(color));
        }
    }
}

/// Render a logo grid to PNG bytes.
fn render_grid(grid: &[[u8; 16]; 16]) -> Vec<u8> {
    let size = GRID * SCALE;
    let mut img = RgbaImage::new(size, size);

    for (y, row) in grid.iter().enumerate() {
        for (x, &idx) in row.iter().enumerate() {
            put_scaled(&mut img, x as u32, y as u32, SCALE, PALETTE[idx as usize]);
        }
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encoding of logo failed");
    buf.into_inner()
}

/// Generate the built-in kittyview kitten logo as PNG bytes.
pub fn generate_logo_png() -> Vec<u8> {
    render_grid(&LOGO)
}

/// Generate an animated logo: the kitten with a speech bubble.
/// Alternates between normal and happy (^_^) expressions with a gentle bounce.
/// Uses SVG rendering for proper text in the speech bubble.
pub fn generate_animated_logo() -> Vec<(Vec<u8>, u32)> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    let normal_b64 = STANDARD.encode(render_grid(&LOGO));
    let happy_b64 = STANDARD.encode(render_grid(&LOGO_HAPPY));

    // Load fonts once for all frames
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    opt.fontdb_mut().set_sans_serif_family("Liberation Sans");
    opt.fontdb_mut().set_serif_family("Liberation Serif");
    opt.fontdb_mut().set_monospace_family("Liberation Mono");

    // (kitten_b64, y_offset, delay_ms)
    let frame_specs: [(&str, u32, u32); 4] = [
        (&normal_b64, 12, 400),
        (&normal_b64, 8, 400),
        (&happy_b64, 12, 600),
        (&happy_b64, 8, 600),
    ];

    let mut frames = Vec::with_capacity(frame_specs.len());

    for (kitten_b64, y, delay) in &frame_specs {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg"
     xmlns:xlink="http://www.w3.org/1999/xlink"
     width="480" height="150">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0%" stop-color="#1a1147"/>
      <stop offset="100%" stop-color="#0d0a2e"/>
    </linearGradient>
  </defs>
  <rect width="480" height="150" rx="12" fill="url(#bg)"/>
  <image href="data:image/png;base64,{kitten_b64}" x="15" y="{y}" width="120" height="120"/>
  <rect x="155" y="22" width="310" height="100" rx="16" fill="white" opacity="0.95"/>
  <polygon points="150,58 163,48 163,68" fill="white" opacity="0.95"/>
  <text x="180" y="60" font-size="26" fill="#2D1B69" font-family="sans-serif" font-weight="bold">kittyview</text>
  <text x="180" y="92" font-size="14" fill="#7B6FC4" font-family="sans-serif">github.com/gominimal/kittyview</text>
</svg>"##
        );

        let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &opt)
            .expect("animated logo SVG parse failed");
        let size = tree.size().to_int_size();
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(size.width(), size.height()).expect("pixmap alloc");
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::default(),
            &mut pixmap.as_mut(),
        );
        let png = pixmap.encode_png().expect("PNG encode");
        frames.push((png, *delay));
    }

    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_produces_valid_png() {
        let png = generate_logo_png();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn logo_dimensions_are_128x128() {
        let png = generate_logo_png();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), GRID * SCALE);
        assert_eq!(img.height(), GRID * SCALE);
        assert_eq!(img.width(), 128);
    }

    #[test]
    fn logo_palette_indices_in_range() {
        for (y, row) in LOGO.iter().enumerate() {
            for (x, &idx) in row.iter().enumerate() {
                assert!(
                    (idx as usize) < PALETTE.len(),
                    "LOGO[{y}][{x}] = {idx} is out of palette range"
                );
            }
        }
    }

    #[test]
    fn happy_logo_palette_indices_in_range() {
        for (y, row) in LOGO_HAPPY.iter().enumerate() {
            for (x, &idx) in row.iter().enumerate() {
                assert!(
                    (idx as usize) < PALETTE.len(),
                    "LOGO_HAPPY[{y}][{x}] = {idx} is out of palette range"
                );
            }
        }
    }

    #[test]
    fn logo_is_horizontally_symmetric() {
        for (y, row) in LOGO.iter().enumerate() {
            let w = row.len();
            for x in 0..w / 2 {
                let left = row[x];
                let right = row[w - 1 - x];
                if left == 0 || left == 2 {
                    assert_eq!(
                        left, right,
                        "row {y}: outline/transparent asymmetry at x={x}"
                    );
                }
            }
        }
    }

    #[test]
    fn animated_logo_produces_multiple_frames() {
        let frames = generate_animated_logo();
        assert_eq!(frames.len(), 4);
        for (png, delay) in &frames {
            assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
            assert!(*delay > 0);
        }
    }

    #[test]
    fn animated_logo_frame_dimensions() {
        let frames = generate_animated_logo();
        let img = image::load_from_memory(&frames[0].0).unwrap();
        assert_eq!(img.width(), 480);
        assert_eq!(img.height(), 150);
    }
}
