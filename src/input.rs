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

    pub fn update_input(&mut self, value: Value) {
        if self.mode == InputMode::Search {
            if let Some(left_brace_index) = value.editable_text.find('{') {
                let prefix = Value {
                    editable_text: value.editable_text[..left_brace_index].to_string(),
                    insertion_policy: value.insertion_policy,
                };
                let suffix = value.editable_text[left_brace_index..].to_string();

                self.value = prefix;
                self.rerank();
                self.value = self.current();
                self.mode = InputMode::Edit;
                self.results.clear();
                self.selected_index = None;
                self.value.editable_text.push_str(&suffix);
                return;
            }
        }

        self.value = value;

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
        let value = if self.value.editable_text.is_empty() {
            Value::raw("")
        } else {
            self.current()
        };

        self.mode = InputMode::Edit;
        self.value = value;
        self.results.clear();
        self.selected_index = None;
    }

    pub fn mode(&self) -> InputMode {
        self.mode
    }

    pub fn value(&self) -> Value {
        self.value.clone()
    }

    pub fn current(&self) -> Value {
        if self.mode == InputMode::Edit {
            return self.value.clone();
        }

        match self.selected() {
            Some(selected)
                if !self
                    .value
                    .editable_text
                    .starts_with(&selected.editable_text) =>
            {
                selected
            }
            _ => self.value.clone(),
        }
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
    fn tilde_with_search_input_seeds_edit_mode_from_selected_match() {
        let mut state = InputState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/me/Documents/research"), 'd'),
            Candidate::new(Value::raw("firefox"), 'c'),
        ]);
        state.update_input(Value::raw(";d"));

        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::escaped("/home/me/Documents/research"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn search_input_without_prefix_resolves_to_selected_match() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.update_input(Value::raw("fir"));

        assert_eq!(state.current(), Value::raw("firefox"));
    }

    #[test]
    fn search_input_extending_selected_match_resolves_to_raw_input() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.update_input(Value::raw("firefox --private-window"));

        assert_eq!(state.current(), Value::raw("firefox --private-window"));
    }

    #[test]
    fn search_input_with_no_matches_resolves_to_raw_input() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        assert_eq!(state.current(), Value::raw("ps aux | grep firefox"));
    }

    #[test]
    fn edit_mode_current_value_is_input_value() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::escaped("/home/me/Documents"), 'd')]);
        state.update_input(Value::raw(";d"));
        state.press_tilde();
        state.update_input(Value::escaped("/home/me/Documents/paper.pdf"));

        assert_eq!(
            state.current(),
            Value::escaped("/home/me/Documents/paper.pdf")
        );
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
        state.update_input(Value::raw(";c"));

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn character_input_with_no_matches_clears_selection() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.update_input(Value::raw("zzz"));

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
        state.update_input(Value::raw(";c"));

        assert_eq!(state.selected(), Some(Value::raw("calculator")));
    }

    #[test]
    fn character_input_in_edit_mode_edits_value_and_ignores_results() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.press_tilde();
        state.update_input(Value::raw("f"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("f"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn tilde_with_empty_input_ignores_selected_match() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn tilde_with_no_selected_match_keeps_typed_raw_input() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c')]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("ps aux | grep firefox"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn left_brace_in_search_mode_enters_edit_mode_from_current_value() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c')]);
        state.update_input(Value::raw("mv "));

        state.update_input(Value::raw("mv {"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("mv {"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn left_brace_in_edit_mode_updates_input_without_search_resolution() {
        let mut state = InputState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c')]);
        state.press_tilde();

        state.update_input(Value::raw("{"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("{"));
        assert_eq!(state.selected(), None);
    }
}
