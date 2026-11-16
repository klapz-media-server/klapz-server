//! The four colors of an item's blurred backdrop, which Plex calls `UltraBlur`.
//!
//! Drawn from the item's background art by the reference's analysis and imported with the
//! library; a client blends them into the backdrop it shows behind the item.

/// An sRGB color, 8 bits a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb([u8; 3]);

impl Rgb {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue])
    }

    /// From `0xRRGGBB`; higher bits are ignored.
    #[must_use]
    pub const fn from_packed(value: u32) -> Self {
        let [_, red, green, blue] = value.to_be_bytes();
        Self([red, green, blue])
    }

    /// `0xRRGGBB`.
    #[must_use]
    pub const fn packed(self) -> u32 {
        let [red, green, blue] = self.0;
        u32::from_be_bytes([0, red, green, blue])
    }

    #[must_use]
    pub const fn channels(self) -> [u8; 3] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CornerColors {
    pub top_left: Rgb,
    pub top_right: Rgb,
    pub bottom_right: Rgb,
    pub bottom_left: Rgb,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_red_highest() {
        let color = Rgb::from_packed(0x001a_2c53);
        assert_eq!(color.channels(), [0x1a, 0x2c, 0x53]);
        assert_eq!(color.packed(), 0x001a_2c53);
        assert_eq!(Rgb::from_packed(0xff1a_2c53), color);
    }
}
