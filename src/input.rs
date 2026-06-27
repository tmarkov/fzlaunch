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
    candidates: Vec<Candidate>,
    results: Vec<Value>,
    selected_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    value: Value,
    match_char: char,
}

impl Candidate {
    pub fn new(value: Value, match_char: char) -> Self {
        Self { value, match_char }
    }

    fn haystack(&self) -> String {
        format!(";{} {}", self.match_char, self.value.editable_text)
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mode: InputMode::Search,
            value: Value::raw(""),
            candidates: Vec::new(),
            results: Vec::new(),
            selected_index: None,
        }
    }
}

impl InputState {
    pub fn feed(&mut self, candidates: impl IntoIterator<Item = Candidate>) {
        self.candidates.extend(candidates);
        self.rerank();
    }

    pub fn type_char(&mut self, ch: char) {
        self.value.editable_text.push(ch);

        if self.mode == InputMode::Search {
            self.rerank();
        }
    }

    fn rerank(&mut self) {
        if self.value.editable_text.is_empty() {
            self.results = self
                .candidates
                .iter()
                .map(|candidate| candidate.value.clone())
                .collect();
        } else {
            let haystacks = self
                .candidates
                .iter()
                .map(Candidate::haystack)
                .collect::<Vec<_>>();

            self.results = frizbee::match_list(
                &self.value.editable_text,
                &haystacks,
                &frizbee::Config::default(),
            )
            .into_iter()
            .map(|matched| self.candidates[matched.index as usize].value.clone())
            .collect();
        }

        self.selected_index = (!self.results.is_empty()).then_some(0);
    }

    pub fn select_next(&mut self) {
        let Some(index) = self.selected_index else {
            return;
        };

        if index + 1 < self.results.len() {
            self.selected_index = Some(index + 1);
        }
    }

    pub fn select_previous(&mut self) {
        let Some(index) = self.selected_index else {
            return;
        };

        if index > 0 {
            self.selected_index = Some(index - 1);
        }
    }

    pub fn press_tilde(&mut self) {
        self.mode = InputMode::Edit;
        self.value = Value::raw("");
        self.results.clear();
        self.selected_index = None;
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn value(&self) -> Value {
        self.value.clone()
    }

    pub fn selected(&self) -> Option<Value> {
        self.selected_index
            .and_then(|index| self.results.get(index))
            .cloned()
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
            Candidate::new(Value::raw("firefox"), 'c'),
            Candidate::new(Value::escaped("/home/me/Documents/research"), 'd'),
        ]);

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn feeding_new_candidates_resets_selection_to_first_match() {
        let mut state = InputState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c'),
            Candidate::new(Value::raw("second"), 'c'),
        ]);
        state.select_next();
        assert_eq!(state.selected(), Some(Value::raw("second")));

        state.feed([
            Candidate::new(Value::raw("new-first"), 'c'),
            Candidate::new(Value::raw("new-second"), 'c'),
        ]);

        assert_eq!(state.selected(), Some(Value::raw("first")));
    }

    #[test]
    fn selection_can_move_back_to_previous_match() {
        let mut state = InputState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c'),
            Candidate::new(Value::raw("second"), 'c'),
        ]);
        state.select_next();
        state.select_previous();

        assert_eq!(state.selected(), Some(Value::raw("first")));
    }

    #[test]
    fn selection_stays_at_last_match_when_moving_down_past_end() {
        let mut state = InputState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c'),
            Candidate::new(Value::raw("second"), 'c'),
        ]);
        state.select_next();
        state.select_next();

        assert_eq!(state.selected(), Some(Value::raw("second")));
    }

    #[test]
    fn selection_stays_at_first_match_when_moving_up_past_start() {
        let mut state = InputState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c'),
            Candidate::new(Value::raw("second"), 'c'),
        ]);
        state.select_previous();

        assert_eq!(state.selected(), Some(Value::raw("first")));
    }

    #[test]
    fn character_input_reranks_candidates_by_haystack_and_resets_selection() {
        let mut state = InputState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/user/files/firefox"), 'f'),
            Candidate::new(Value::raw("firefox"), 'c'),
        ]);
        state.type_char(';');
        state.type_char('c');

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn character_input_with_no_matches_clears_selection() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.type_char('z');
        state.type_char('z');
        state.type_char('z');

        assert_eq!(state.selected(), None);
    }

    #[test]
    fn feed_appends_candidate_matches_from_multiple_sources() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("calculator"), 'c')]);
        state.feed([Candidate::new(
            Value::escaped("/home/user/files/paper.pdf"),
            'f',
        )]);
        state.type_char(';');
        state.type_char('c');

        assert_eq!(state.selected(), Some(Value::raw("calculator")));
    }

    #[test]
    fn character_input_in_edit_mode_edits_value_and_ignores_results() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.press_tilde();
        state.type_char('f');

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("f"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn initial_tilde_clears_existing_results() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.selected(), None);
    }
}
