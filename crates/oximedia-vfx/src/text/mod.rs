//! Text rendering and animation effects.

pub mod animate;
pub mod font;
pub mod layout;
pub mod paint;
pub mod render;

pub use animate::{AnimationType, TextAnimation};
pub use font::{FontFace, GlyphBitmap, GlyphCache};
pub use layout::{CharAdvance, LineMetrics, PlacedGlyph, TextAlign, TextLayout};
pub use paint::CoverageMask;
pub use render::{TextConfig, TextRenderer};
