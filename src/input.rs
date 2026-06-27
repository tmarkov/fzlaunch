use crate::model::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Search,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputState {
    mode: InputMode,
    value: Value,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mode: InputMode::Search,
            value: Value::raw(""),
        }
    }
}

impl InputState {
    pub fn press_tilde(&mut self) {
        self.mode = InputMode::Edit;
        self.value = Value::raw("");
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn value(&self) -> Value {
        self.value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_tilde_enters_edit_mode_with_empty_raw_buffer() {
        let mut state = InputState::default();

        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(""));
    }

    #[test]
    fn feed_adds_candidate_matches_and_selects_first_by_default() {
        let mut state = InputState::default();

        state.feed([
            Value::raw("firefox"),
            Value::escaped("/home/me/Documents/research"),
        ]);

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }
}
