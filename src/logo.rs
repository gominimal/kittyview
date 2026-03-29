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

// 16x16 pixel-art kitten — compiler checks both dimensions
// Kitten proportions: tall ears, big eyes, prominent blush
#[rustfmt::skip]
const LOGO: [[u8; 16]; 16] = [
//   0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5
    [0,0,0,2,0,0,0,0,0,0,0,0,2,0,0,0], //  0  ear tips
    [0,0,2,4,2,0,0,0,0,0,0,2,4,2,0,0], //  1  ears (pink inner)
    [0,2,1,4,2,0,0,0,0,0,0,2,4,1,2,0], //  2  ears wider (pink inner)
    [0,2,1,1,2,2,2,2,2,2,2,2,1,1,2,0], //  3  ears → head
    [0,2,1,1,1,1,1,1,1,1,1,1,1,1,2,0], //  4  forehead
    [0,2,1,2,3,2,1,1,1,1,2,3,2,1,2,0], //  5  eyes  (D·W·D / D·W·D)
    [0,2,1,2,2,2,1,1,1,1,2,2,2,1,2,0], //  6  eyes  (D·D·D / D·D·D)
    [0,2,1,4,4,1,1,1,1,1,1,4,4,1,2,0], //  7  blush (2px per cheek)
    [0,2,1,1,1,1,1,4,4,1,1,1,1,1,2,0], //  8  nose
    [0,2,1,1,1,1,2,1,1,2,1,1,1,1,2,0], //  9  mouth (ω top)
    [0,2,1,1,1,1,1,2,2,1,1,1,1,1,2,0], // 10  mouth (ω bottom)
    [0,0,2,1,1,1,1,1,1,1,1,1,1,2,0,0], // 11  chin narrows
    [0,0,0,2,1,1,1,1,1,1,1,1,2,0,0,0], // 12  lower chin
    [0,0,0,0,2,2,2,2,2,2,2,2,0,0,0,0], // 13  bottom edge
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], // 14  blank
    [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0], // 15  blank
];

/// Generate the built-in kittyview kitten logo as PNG bytes.
pub fn generate_logo_png() -> Vec<u8> {
    let size = GRID * SCALE;
    let mut img = RgbaImage::new(size, size);

    for (y, row) in LOGO.iter().enumerate() {
        for (x, &idx) in row.iter().enumerate() {
            let color = image::Rgba(PALETTE[idx as usize]);
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    img.put_pixel(x as u32 * SCALE + dx, y as u32 * SCALE + dy, color);
                }
            }
        }
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png)
        .expect("PNG encoding of logo failed");
    buf.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_produces_valid_png() {
        let png = generate_logo_png();
        // PNG magic bytes
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
    fn logo_is_horizontally_symmetric() {
        for (y, row) in LOGO.iter().enumerate() {
            let w = row.len();
            for x in 0..w / 2 {
                let left = row[x];
                let right = row[w - 1 - x];
                // Outline (2) and transparent (0) must be symmetric
                if left == 0 || left == 2 {
                    assert_eq!(
                        left, right,
                        "row {y}: outline/transparent asymmetry at x={x}"
                    );
                }
            }
        }
    }
}
