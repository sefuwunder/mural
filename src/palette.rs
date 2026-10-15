//! The 16-color artist palette.

use sdl2::pixels::Color;

pub const COUNT: usize = 16;

#[derive(Clone, Copy)]
pub struct Swatch {
    pub name: &'static str,
    pub color: Color,
}

pub const PALETTE: [Swatch; COUNT] = [
    Swatch { name: "Ink",        color: Color::RGB(0x1c, 0x1a, 0x17) },
    Swatch { name: "Charcoal",   color: Color::RGB(0x4a, 0x45, 0x40) },
    Swatch { name: "Stone",      color: Color::RGB(0x8a, 0x83, 0x78) },
    Swatch { name: "Paper",      color: Color::RGB(0xf5, 0xf2, 0xea) },
    Swatch { name: "Crimson",    color: Color::RGB(0xc2, 0x2f, 0x3d) },
    Swatch { name: "Vermilion",  color: Color::RGB(0xe0, 0x51, 0x2a) },
    Swatch { name: "Amber",      color: Color::RGB(0xe8, 0xa1, 0x3c) },
    Swatch { name: "Lemon",      color: Color::RGB(0xf2, 0xd2, 0x4b) },
    Swatch { name: "Leaf",       color: Color::RGB(0x5a, 0x9e, 0x4b) },
    Swatch { name: "Teal",       color: Color::RGB(0x2f, 0x8f, 0x83) },
    Swatch { name: "Sky",        color: Color::RGB(0x4a, 0x9e, 0xd6) },
    Swatch { name: "Ultramarine",color: Color::RGB(0x2f, 0x4f, 0xc2) },
    Swatch { name: "Violet",     color: Color::RGB(0x7a, 0x4f, 0xc2) },
    Swatch { name: "Magenta",    color: Color::RGB(0xc2, 0x43, 0x9e) },
    Swatch { name: "Umber",      color: Color::RGB(0x7a, 0x52, 0x30) },
    Swatch { name: "Rose",       color: Color::RGB(0xe0, 0x8a, 0x97) },
];

pub const PAPER: Color = Color::RGB(0xd8, 0xd3, 0xc5);
pub const PAPER_DOT: Color = Color::RGB(0xc4, 0xbd, 0xac);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sixteen_colors() {
        assert_eq!(PALETTE.len(), 16);
        assert_eq!(COUNT, 16);
    }
}
