//! Hand-crafted 5×7 pixel font for the hex-readout glyphs.
//!
//! Only the 17 characters we ever need (`#0123456789ABCDEF`) are encoded.
//! Each glyph is stored as 7 rows where the low 5 bits of each byte are the
//! pixel mask (bit 4 = leftmost column, bit 0 = rightmost). At ~140 bytes
//! total this is far smaller than a TTF and renders crisp at any integer
//! scale, which fits the picker's pixel-precise vibe.

pub const GLYPH_W: u32 = 5;
pub const GLYPH_H: u32 = 7;
pub const SPACING: u32 = 1;

const FONT: &[(char, [u8; 7])] = &[
    ('#', [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010]),
    ('0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
    ('1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    ('2', [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111]),
    ('3', [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110]),
    ('4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
    ('5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
    ('6', [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
    ('7', [0b11111, 0b00001, 0b00010, 0b00010, 0b00100, 0b00100, 0b00100]),
    ('8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
    ('9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100]),
    ('A', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    ('B', [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110]),
    ('C', [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110]),
    ('D', [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110]),
    ('E', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
    ('F', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000]),
    // Added for the Screen Ruler label ("120 X 80").
    ('X', [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001]),
    (' ', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
];

fn glyph(c: char) -> Option<&'static [u8; 7]> {
    FONT.iter().find(|(g, _)| *g == c).map(|(_, b)| b)
}

/// Width in pixels of a string at the given integer scale, including
/// inter-character spacing.
pub fn text_width(s: &str, scale: u32) -> u32 {
    let n = s.chars().count() as u32;
    if n == 0 {
        return 0;
    }
    n * GLYPH_W * scale + (n - 1) * SPACING * scale
}

pub fn text_height(scale: u32) -> u32 {
    GLYPH_H * scale
}

/// Renders `text` into `canvas` (BGRA layout) at top-left `(x, y)` with the
/// given solid `[r, g, b]` colour and integer pixel `scale`. Pixels outside
/// the canvas are silently clipped.
pub fn draw_text(
    canvas: &mut [u8],
    cw: u32,
    ch: u32,
    mut x: i32,
    y: i32,
    text: &str,
    color: [u8; 3],
    scale: u32,
) {
    for c in text.chars() {
        if let Some(rows) = glyph(c) {
            for (row_i, &row) in rows.iter().enumerate() {
                for col in 0..GLYPH_W {
                    if (row >> (GLYPH_W - 1 - col)) & 1 == 0 {
                        continue;
                    }
                    // Stamp a scale × scale block for this pixel.
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = x + (col * scale + dx) as i32;
                            let py = y + (row_i as u32 * scale + dy) as i32;
                            if px < 0 || px >= cw as i32 || py < 0 || py >= ch as i32 {
                                continue;
                            }
                            let di = ((py * cw as i32 + px) * 4) as usize;
                            canvas[di] = color[2];
                            canvas[di + 1] = color[1];
                            canvas[di + 2] = color[0];
                            canvas[di + 3] = 0xFF;
                        }
                    }
                }
            }
        }
        x += (GLYPH_W * scale + SPACING * scale) as i32;
    }
}
