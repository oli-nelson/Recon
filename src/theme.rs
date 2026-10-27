use crate::{Theme, animation};
use ratatui::style::Color;

pub struct Palette {
    pub foreground: Color,
    pub muted: Color,
    pub accent: Color,
    pub positive: Color,
    pub negative: Color,
    pub gold: Color,
    age: [Color; 4],
}
impl Theme {
    pub fn palette(self) -> Palette {
        let (foreground, muted, accent, positive, negative, gold, age) = match self {
            Self::Dark => (
                (204, 214, 237),
                (113, 128, 155),
                (106, 173, 255),
                (100, 210, 160),
                (235, 122, 139),
                (242, 195, 107),
                [(94, 234, 212), (106, 173, 255), (242, 195, 107)],
            ),
            Self::Light => (
                (35, 43, 60),
                (100, 110, 128),
                (106, 173, 255),
                (100, 210, 160),
                (235, 122, 139),
                (242, 195, 107),
                [(0, 128, 124), (47, 103, 189), (158, 100, 32)],
            ),
            // Oli's installed Ashen palette: grey foreground, ember/orange
            // accents, muted teal and green; retain his transparent background.
            Self::Ashen => (
                (180, 180, 180),
                (138, 143, 152),
                (196, 105, 61),
                (98, 156, 125),
                (223, 100, 100),
                (229, 167, 42),
                [(216, 124, 74), (228, 154, 68), (74, 139, 139)],
            ),
        };
        let rgb = |(r, g, b)| Color::Rgb(r, g, b);
        Palette {
            foreground: rgb(foreground),
            muted: rgb(muted),
            accent: rgb(accent),
            positive: rgb(positive),
            negative: rgb(negative),
            gold: rgb(gold),
            age: [rgb(age[0]), rgb(age[1]), rgb(age[2]), rgb(muted)],
        }
    }
}
impl Palette {
    pub fn recency(&self, age: f64) -> Color {
        animation::gradient(age, self.age)
    }
}
