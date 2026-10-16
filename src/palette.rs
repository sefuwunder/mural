//! The 16-color artist palette.

use sdl2::pixels::Color;

pub const COUNT: usize = 16;

#[derive(Clone, Copy)]
pub struct Swatch {
    pub color: Color,
}

pub const PALETTE: [Swatch; COUNT] = [
    Swatch { color: Color::RGB(0x1c, 0x1a, 0x17) },
    Swatch { color: Color::RGB(0x4a, 0x45, 0x40) },
    Swatch { color: Color::RGB(0x8a, 0x83, 0x78) },
    Swatch { color: Color::RGB(0xf5, 0xf2, 0xea) },
    Swatch { color: Color::RGB(0xc2, 0x2f, 0x3d) },
    Swatch { color: Color::RGB(0xe0, 0x51, 0x2a) },
    Swatch { color: Color::RGB(0xe8, 0xa1, 0x3c) },
    Swatch { color: Color::RGB(0xf2, 0xd2, 0x4b) },
    Swatch { color: Color::RGB(0x5a, 0x9e, 0x4b) },
    Swatch { color: Color::RGB(0x2f, 0x8f, 0x83) },
    Swatch { color: Color::RGB(0x4a, 0x9e, 0xd6) },
    Swatch { color: Color::RGB(0x2f, 0x4f, 0xc2) },
    Swatch { color: Color::RGB(0x7a, 0x4f, 0xc2) },
    Swatch { color: Color::RGB(0xc2, 0x43, 0x9e) },
    Swatch { color: Color::RGB(0x7a, 0x52, 0x30) },
    Swatch { color: Color::RGB(0xe0, 0x8a, 0x97) },
];

pub const PAPER: Color = Color::RGB(0xd8, 0xd3, 0xc5);

/// Built-in palette as plain RGB triples (for serde defaults / doc storage).
pub fn default_rgb() -> [[u8; 3]; COUNT] {
    let mut out = [[0u8; 3]; COUNT];
    let mut i = 0;
    while i < COUNT {
        let c = PALETTE[i].color;
        out[i] = [c.r, c.g, c.b];
        i += 1;
    }
    out
}
pub const PAPER_DOT: Color = Color::RGB(0xc4, 0xbd, 0xac);

/// An interchangeable 16-color set: a display name plus the colors.
pub struct ColorSet {
    pub name: &'static str,
    pub colors: [[u8; 3]; COUNT],
    pub names: [&'static str; COUNT],
}

/// Five interchangeable color sets. Index 0 ("Studio") is the original
/// artist palette and the default for new documents and legacy files.
pub const COLOR_SETS: [ColorSet; 5] = [
    ColorSet {
        name: "Studio",
        colors: [
            [0x1c, 0x1a, 0x17], [0x4a, 0x45, 0x40], [0x8a, 0x83, 0x78], [0xf5, 0xf2, 0xea],
            [0xc2, 0x2f, 0x3d], [0xe0, 0x51, 0x2a], [0xe8, 0xa1, 0x3c], [0xf2, 0xd2, 0x4b],
            [0x5a, 0x9e, 0x4b], [0x2f, 0x8f, 0x83], [0x4a, 0x9e, 0xd6], [0x2f, 0x4f, 0xc2],
            [0x7a, 0x4f, 0xc2], [0xc2, 0x43, 0x9e], [0x7a, 0x52, 0x30], [0xe0, 0x8a, 0x97],
        ],
        names: [
            "Ink", "Charcoal", "Stone", "Paper", "Crimson", "Vermilion", "Amber", "Lemon",
            "Leaf", "Teal", "Sky", "Ultramarine", "Violet", "Magenta", "Umber", "Rose",
        ],
    },
    ColorSet {
        name: "Dutch Masters",
        colors: [
            [0x14, 0x11, 0x0e], [0x2e, 0x21, 0x18], [0x5a, 0x3a, 0x22], [0x7b, 0x5b, 0x3a],
            [0xa6, 0x7c, 0x3d], [0xc9, 0x98, 0x3f], [0xe0, 0xb4, 0x5c], [0xed, 0xe0, 0xc8],
            [0x8e, 0x2f, 0x1b], [0xb5, 0x50, 0x2a], [0x6e, 0x1f, 0x1f], [0x2f, 0x3b, 0x5c],
            [0x4a, 0x5d, 0x3a], [0x8c, 0x85, 0x78], [0xd9, 0xc9, 0xa8], [0xf0, 0xe6, 0xd2],
        ],
        names: [
            "Umbra", "Burnt Umber", "Raw Sienna", "Yellow Ochre", "Gold Ochre", "Gamboge",
            "Lead-Tin", "Chalk", "Madder", "Vermilion", "Alizarin", "Slate", "Terre Verte",
            "Ash Grey", "Naples", "Flake White",
        ],
    },
    ColorSet {
        name: "Pop",
        colors: [
            [0xff, 0x00, 0x6e], [0xff, 0x2e, 0x88], [0xff, 0xd5, 0x00], [0xff, 0x6b, 0x00],
            [0xee, 0x1c, 0x25], [0x00, 0xae, 0xef], [0x00, 0x57, 0xb8], [0x00, 0xc8, 0x53],
            [0x7b, 0x2f, 0xf7], [0x11, 0x11, 0x11], [0xff, 0xff, 0xff], [0xff, 0x8a, 0x00],
            [0x00, 0xe5, 0xcc], [0xc6, 0xff, 0x00], [0xff, 0x4d, 0x6d], [0x2b, 0x2b, 0x2b],
        ],
        names: [
            "Hot Pink", "Shocking", "Taxi", "Ember", "Pop Red", "Day-Glo", "Cobalt",
            "Acid Green", "Purple Haze", "Black", "White", "Orange Pop", "Teal Pop",
            "Neon", "Rose Pop", "Charcoal",
        ],
    },
    ColorSet {
        name: "Picasso",
        colors: [
            [0x1b, 0x3a, 0x5c], [0x2e, 0x5a, 0x88], [0x4a, 0x7b, 0xa7], [0x7f, 0xa8, 0xc9],
            [0xc9, 0xd6, 0xe3], [0xe8, 0xb4, 0xa0], [0xd9, 0x8e, 0x73], [0xb5, 0x6a, 0x4c],
            [0x8c, 0x4a, 0x32], [0x3b, 0x2f, 0x2f], [0x14, 0x14, 0x14], [0xed, 0xed, 0xed],
            [0xc9, 0xa2, 0x27], [0x7a, 0x6a, 0x53], [0x5c, 0x4a, 0x3a], [0xa6, 0x3d, 0x2f],
        ],
        names: [
            "Blue Period", "Steel Blue", "Harlequin", "Pale Blue", "Porcelain", "Rose Nude",
            "Terracotta", "Sienna Rose", "Burnt Rose", "Sepia", "Noir", "Gesso", "Ochre Gold",
            "Taupe", "Umber", "Oxide Red",
        ],
    },
    ColorSet {
        name: "Basquiat",
        colors: [
            [0xff, 0xd2, 0x00], [0xff, 0x4d, 0x00], [0xd9, 0x04, 0x29], [0x11, 0x13, 0x18],
            [0xf2, 0xed, 0xe3], [0x0f, 0xa3, 0xa3], [0x1d, 0x4e, 0xd8], [0xff, 0x8c, 0x00],
            [0xec, 0x48, 0x99], [0x22, 0xc5, 0x5e], [0xa1, 0x62, 0x07], [0x78, 0x35, 0x0f],
            [0xe5, 0xe5, 0xe5], [0x3b, 0x82, 0xf6], [0xfa, 0xcc, 0x15], [0x7c, 0x2d, 0x12],
        ],
        names: [
            "Crown Gold", "SAMO Orange", "Crimson", "Asphalt", "Bone", "Teal", "Royal",
            "Tag Orange", "Pink", "Green", "Ochre", "Dirt", "Concrete", "Blue", "Yellow", "Rust",
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sixteen_colors() {
        assert_eq!(PALETTE.len(), 16);
        assert_eq!(COUNT, 16);
    }
    #[test]
    fn five_sets_of_sixteen() {
        assert_eq!(COLOR_SETS.len(), 5);
        for s in &COLOR_SETS {
            assert_eq!(s.colors.len(), 16);
            assert_eq!(s.names.len(), 16);
            assert!(!s.name.is_empty());
        }
        // set 0 ("Studio") must match the legacy built-in palette
        assert_eq!(COLOR_SETS[0].name, "Studio");
        assert_eq!(COLOR_SETS[0].colors, default_rgb());
        // spot-check a few hex values from the spec
        assert_eq!(COLOR_SETS[1].colors[0], [0x14, 0x11, 0x0e]); // Dutch Umbra
        assert_eq!(COLOR_SETS[2].colors[0], [0xff, 0x00, 0x6e]); // Pop Hot Pink
        assert_eq!(COLOR_SETS[3].colors[0], [0x1b, 0x3a, 0x5c]); // Picasso Blue Period
        assert_eq!(COLOR_SETS[4].colors[0], [0xff, 0xd2, 0x00]); // Basquiat Crown Gold
    }
}
