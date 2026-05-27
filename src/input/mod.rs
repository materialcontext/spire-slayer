pub mod event;
pub mod manual;

pub use event::{spawn_event_loop, AppEvent};
pub use manual::{default_combat_state, ManualInputState};
