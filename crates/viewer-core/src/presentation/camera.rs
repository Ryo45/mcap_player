use crate::{CameraId, CameraStatus};
use std::fmt;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum OverlayStatus {
    #[default]
    Waiting,
    Ready {
        visible_points: usize,
    },
    Error(String),
}

impl fmt::Display for OverlayStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Waiting => formatter.write_str("plan waiting"),
            Self::Ready { visible_points } => {
                write!(formatter, "plan {visible_points} visible pts")
            }
            Self::Error(error) => formatter.write_str(error),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraPresentation {
    pub id: CameraId,
    pub topic: String,
    pub status: CameraStatus,
    pub fps: f64,
    pub overlay: OverlayStatus,
    pub focused: bool,
}
