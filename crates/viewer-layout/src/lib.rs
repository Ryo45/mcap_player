mod error;
mod model;
mod validation;

pub use error::{LayoutLoadError, LayoutSaveError, LayoutValidationError, PanelIdError};
pub use model::{
    CURRENT_LAYOUT_SCHEMA_VERSION, LayoutDocument, LayoutNode, PanelId, PanelNode, SplitChild,
    SplitDirection,
};
pub use validation::{MAX_LAYOUT_DEPTH, MAX_PANEL_COUNT};
