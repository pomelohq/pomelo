pub mod buffer;
pub mod display;
pub mod fold;
pub mod highlight;
pub mod language;
pub mod movement;
pub mod syntax;
pub mod theme;
pub mod transform;
pub mod wrap;

pub use buffer::EditorBuffer;
pub use highlight::Lang;
pub use syntax::Syntax;
pub use theme::Theme;
