pub mod desktop_index;
pub mod focus_or_launch;

pub use desktop_index::{AppEntry, AppIndex};
pub use focus_or_launch::{focus_or_launch, FocusOrLaunchResult, FocusTool};
