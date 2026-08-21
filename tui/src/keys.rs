//! Keymap.
//!
//! Owner rule (carried over from the Go TUI, where it was pin-tested): never
//! bind a bare letter. Typing "How…" once toggled the hook trace. Every
//! action below needs Control or Alt; plain keys always go to the composer.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Ctrl+C while a turn runs; Ctrl+Q or Ctrl+C when idle.
    Quit,
    /// Ctrl+C while a turn runs.
    CancelTurn,
    /// Enter.
    Send,
    /// Shift+Enter, Alt+Enter, Ctrl+J.
    Newline,
    /// Esc: close an overlay or clear the composer.
    Escape,
    /// Ctrl+O.
    OpenModelPicker,
    /// Ctrl+P.
    OpenProviderPicker,
    /// Ctrl+T.
    ToggleToolFold,
    /// Ctrl+H.
    ToggleHooks,
    /// Ctrl+A: master debug/verbose toggle (hooks, unfolded tool output,
    /// internal kernel events).
    ToggleDebug,
    /// Ctrl+E.
    ToggleEventFeed,
    /// Ctrl+G.
    ToggleSubagents,
    /// Ctrl+S: steer the running turn with the composer text.
    Steer,
    /// Ctrl+X: drop the queued message.
    DropQueued,
    /// Ctrl+B: drop the last attachment chip.
    DropChip,
    /// Ctrl+V: voice record toggle.
    Voice,
    /// Ctrl+D: dump the transcript to a file.
    DumpTranscript,
    /// Ctrl+L: clear the transcript view.
    ClearView,
    /// F1 or Ctrl+/ .
    Help,
    /// Tab: reopen completion for an @-path or /command token.
    Complete,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,
    /// Everything else: hand the key to the composer.
    Compose,
}

pub fn action_for(key: &KeyEvent, turn_running: bool) -> Action {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Char('c') if ctrl => {
            if turn_running {
                Action::CancelTurn
            } else {
                Action::Quit
            }
        }
        KeyCode::Char('q') if ctrl => Action::Quit,
        KeyCode::Char('j') if ctrl => Action::Newline,
        KeyCode::Char('o') if ctrl => Action::OpenModelPicker,
        KeyCode::Char('p') if ctrl => Action::OpenProviderPicker,
        KeyCode::Char('t') if ctrl => Action::ToggleToolFold,
        KeyCode::Char('h') if ctrl => Action::ToggleHooks,
        KeyCode::Char('a') if ctrl => Action::ToggleDebug,
        KeyCode::Char('e') if ctrl => Action::ToggleEventFeed,
        KeyCode::Char('g') if ctrl => Action::ToggleSubagents,
        KeyCode::Char('s') if ctrl => Action::Steer,
        KeyCode::Char('x') if ctrl => Action::DropQueued,
        KeyCode::Char('b') if ctrl => Action::DropChip,
        KeyCode::Char('v') if ctrl => Action::Voice,
        KeyCode::Char('d') if ctrl => Action::DumpTranscript,
        KeyCode::Char('l') if ctrl => Action::ClearView,
        KeyCode::Char('/') if ctrl => Action::Help,
        KeyCode::F(1) => Action::Help,
        KeyCode::Tab => Action::Complete,
        KeyCode::Enter if shift || alt => Action::Newline,
        KeyCode::Enter => Action::Send,
        KeyCode::Esc => Action::Escape,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::Up if ctrl => Action::ScrollUp,
        KeyCode::Down if ctrl => Action::ScrollDown,
        KeyCode::Home if ctrl => Action::ScrollTop,
        KeyCode::End if ctrl => Action::ScrollBottom,
        _ => Action::Compose,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn bare_letters_always_reach_the_composer() {
        for c in "howdyHOWDY1234/?".chars() {
            assert_eq!(
                action_for(&key(KeyCode::Char(c), KeyModifiers::NONE), false),
                Action::Compose,
                "bare {c:?} must not be an action"
            );
            assert_eq!(
                action_for(&key(KeyCode::Char(c), KeyModifiers::SHIFT), false),
                Action::Compose
            );
        }
    }

    #[test]
    fn control_combos_map_and_ctrl_c_depends_on_turn_state() {
        assert_eq!(
            action_for(&key(KeyCode::Char('h'), KeyModifiers::CONTROL), false),
            Action::ToggleHooks
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), true),
            Action::CancelTurn
        );
        assert_eq!(
            action_for(&key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            Action::Quit
        );
        assert_eq!(
            action_for(&key(KeyCode::Enter, KeyModifiers::NONE), false),
            Action::Send
        );
        assert_eq!(
            action_for(&key(KeyCode::Enter, KeyModifiers::SHIFT), false),
            Action::Newline
        );
    }
}
