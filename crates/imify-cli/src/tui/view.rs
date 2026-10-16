//! The state and rendering of one command's live view.

use ratatui::Frame;
use ratatui::text::Text;

/// The live view of one command. State changes only in [`View::apply`]; [`View::render`] and
/// [`View::summary`] only read it.
pub trait View {
    /// The progress event the view consumes.
    type Event;

    /// The rows of the inline viewport. Read once, when the live view starts.
    fn height(&self) -> u16;

    /// Updates the state with one event.
    fn apply(&mut self, event: Self::Event);

    /// Draws the current state into `frame`.
    fn render(&self, frame: &mut Frame);

    /// The lines left in the scrollback when the view ends, finished or interrupted.
    fn summary(&self) -> Text<'static>;
}
