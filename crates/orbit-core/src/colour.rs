//! A colour picked from the screen, written the ways people paste colours.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Colour {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Colour {
    /// "#1E90FF"
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// "rgb(30, 144, 255)"
    pub fn rgb(self) -> String {
        format!("rgb({}, {}, {})", self.r, self.g, self.b)
    }

    /// "hsl(210, 100%, 56%)"
    pub fn hsl(self) -> String {
        let (h, s, l) = self.hsl_parts();
        format!("hsl({h}, {s}%, {l}%)")
    }

    /// Hue in degrees, then saturation and lightness in percent, all rounded.
    pub fn hsl_parts(self) -> (u32, u32, u32) {
        let (r, g, b) = (self.r as f64 / 255.0, self.g as f64 / 255.0, self.b as f64 / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let lightness = (max + min) / 2.0;
        let delta = max - min;
        if delta == 0.0 {
            return (0, 0, (lightness * 100.0).round() as u32);
        }
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if max == r {
            60.0 * ((g - b) / delta).rem_euclid(6.0)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        ((hue.round() as u32) % 360, (saturation * 100.0).round() as u32, (lightness * 100.0).round() as u32)
    }

    /// Whether dark text reads better on it than light text.
    pub fn is_light(self) -> bool {
        // Relative luminance, roughly, as WCAG weighs the channels.
        let luminance = 0.2126 * self.r as f64 + 0.7152 * self.g as f64 + 0.0722 * self.b as f64;
        luminance > 150.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_colour_three_ways() {
        let blue = Colour { r: 30, g: 144, b: 255 };
        assert_eq!(blue.hex(), "#1E90FF");
        assert_eq!(blue.rgb(), "rgb(30, 144, 255)");
        assert_eq!(blue.hsl(), "hsl(210, 100%, 56%)");
        assert_eq!(Colour { r: 0, g: 0, b: 0 }.hsl(), "hsl(0, 0%, 0%)");
        assert_eq!(Colour { r: 255, g: 255, b: 255 }.hsl(), "hsl(0, 0%, 100%)");
        assert_eq!(Colour { r: 255, g: 0, b: 0 }.hsl(), "hsl(0, 100%, 50%)");
        assert_eq!(Colour { r: 128, g: 0, b: 128 }.hsl(), "hsl(300, 100%, 25%)");
    }

    #[test]
    fn knows_light_from_dark() {
        assert!(Colour { r: 255, g: 255, b: 255 }.is_light());
        assert!(!Colour { r: 20, g: 20, b: 40 }.is_light());
    }
}
