use crate::model::{Candidate, Queue, Value};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Search,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherState {
    mode: InputMode,
    value: Value,
    candidates: Vec<Candidate>,
    results: Vec<Candidate>,
    selected_index: Option<usize>,
    queue: Queue,
}

struct IndexedHaystack {
    index: usize,
    haystack: String,
}

impl AsRef<str> for IndexedHaystack {
    fn as_ref(&self) -> &str {
        &self.haystack
    }
}

impl Default for LauncherState {
    fn default() -> Self {
        Self {
            mode: InputMode::Search,
            value: Value::raw(""),
            candidates: Vec::new(),
            results: Vec::new(),
            selected_index: None,
            queue: Queue::new(),
        }
    }
}

impl LauncherState {
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
            self.results = self.candidates.clone();
        } else {
            let haystacks = self
                .candidates
                .iter()
                .map(candidate_haystack)
                .collect::<Vec<_>>();

            let indexed_haystacks = haystacks
                .into_iter()
                .enumerate()
                .map(|(index, haystack)| IndexedHaystack { index, haystack });
            let mut matcher = Matcher::new(Config::DEFAULT.match_paths());

            self.results = Pattern::parse(
                &self.value.editable_text,
                CaseMatching::Ignore,
                Normalization::Smart,
            )
            .match_list(indexed_haystacks, &mut matcher)
            .into_iter()
            .map(|(matched, _)| self.candidates[matched.index].clone())
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

    pub fn press_tab(&mut self) {
        let current = self.current();
        if !current.editable_text.is_empty() {
            self.queue.compose(current);
        }
        self.reset_input();
    }

    pub fn press_enter(&mut self) -> Option<Value> {
        let current = self.current();
        if current.editable_text.is_empty() {
            self.reset_input();
            return None;
        }

        if self.queue.is_empty() {
            if let Some(direct_action) = self
                .selected_index
                .and_then(|index| self.results.get(index))
                .and_then(|candidate| candidate.direct_action().cloned())
            {
                self.queue.compose(direct_action);
            }
        }

        self.queue.compose(current);
        let command = self.queue.compile();

        self.reset_input();

        command
    }

    pub fn queue_status(&self) -> Option<String> {
        self.queue.status()
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
                if selected.editable_text.len() < self.value.editable_text.len()
                    && self
                        .value
                        .editable_text
                        .starts_with(&selected.editable_text) =>
            {
                self.value.clone()
            }
            Some(selected) => selected,
            None => self.value.clone(),
        }
    }

    pub fn selected(&self) -> Option<Value> {
        self.selected_index
            .and_then(|index| self.results.get(index))
            .map(|candidate| candidate.value().clone())
    }

    pub fn results(&self) -> Vec<String> {
        self.results.iter().map(candidate_haystack).collect()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    fn reset_input(&mut self) {
        self.mode = InputMode::Search;
        self.value = Value::raw("");
        self.rerank();
    }
}

fn candidate_haystack(candidate: &Candidate) -> String {
    format!(
        ";{} {}",
        candidate.selector(),
        candidate.value().editable_text
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_tilde_enters_edit_mode_with_empty_raw_buffer() {
        let mut state = LauncherState::default();

        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(""));
    }

    #[test]
    fn tilde_with_search_input_seeds_edit_mode_from_selected_match() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/me/Documents/research"), 'd', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.update_input(Value::raw(";d"));

        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::escaped("/home/me/Documents/research"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn search_input_without_prefix_resolves_to_selected_match() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("fir"));

        assert_eq!(state.current(), Value::raw("firefox"));
    }

    #[test]
    fn search_input_extending_selected_match_resolves_to_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("firefox --private-window"));

        assert_eq!(state.current(), Value::raw("firefox --private-window"));
    }

    #[test]
    fn search_input_equal_to_selected_match_resolves_to_selected_value() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw("/home/me/paper.pdf"));

        assert_eq!(state.current(), Value::escaped("/home/me/paper.pdf"));
    }

    #[test]
    fn search_input_with_no_matches_resolves_to_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        assert_eq!(state.current(), Value::raw("ps aux | grep firefox"));
    }

    #[test]
    fn edit_mode_current_value_is_input_value() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/Documents"),
            'd',
            None,
        )]);
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
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("firefox"), 'c', None),
            Candidate::new(Value::escaped("/home/me/Documents/research"), 'd', None),
        ]);

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn results_show_candidate_haystacks() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("firefox"), 'c', None),
            Candidate::new(Value::escaped("/home/me/paper.pdf"), 'f', None),
        ]);

        assert_eq!(
            state.results(),
            vec![
                ";c firefox".to_string(),
                ";f /home/me/paper.pdf".to_string()
            ]
        );
    }

    #[test]
    fn feeding_new_candidates_resets_selection_to_first_match() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c', None),
            Candidate::new(Value::raw("second"), 'c', None),
        ]);
        state.select_next();
        assert_eq!(state.selected(), Some(Value::raw("second")));

        state.feed([
            Candidate::new(Value::raw("new-first"), 'c', None),
            Candidate::new(Value::raw("new-second"), 'c', None),
        ]);

        assert_eq!(state.selected(), Some(Value::raw("first")));
    }

    #[test]
    fn selection_can_move_back_to_previous_match() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c', None),
            Candidate::new(Value::raw("second"), 'c', None),
        ]);
        state.select_next();
        state.select_previous();

        assert_eq!(state.selected(), Some(Value::raw("first")));
    }

    #[test]
    fn selection_stays_at_last_match_when_moving_down_past_end() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c', None),
            Candidate::new(Value::raw("second"), 'c', None),
        ]);
        state.select_next();
        state.select_next();

        assert_eq!(state.selected(), Some(Value::raw("second")));
    }

    #[test]
    fn selection_stays_at_first_match_when_moving_up_past_start() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c', None),
            Candidate::new(Value::raw("second"), 'c', None),
        ]);
        state.select_previous();

        assert_eq!(state.selected(), Some(Value::raw("first")));
    }

    #[test]
    fn select_next_with_no_results_is_noop() {
        let mut state = LauncherState::default();

        state.select_next();

        assert_eq!(state.selected(), None);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.mode(), InputMode::Search);
    }

    #[test]
    fn select_previous_with_no_results_is_noop() {
        let mut state = LauncherState::default();

        state.select_previous();

        assert_eq!(state.selected(), None);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.mode(), InputMode::Search);
    }

    #[test]
    fn feed_empty_candidates_keeps_selection_empty_without_existing_candidates() {
        let mut state = LauncherState::default();

        state.feed([]);

        assert_eq!(state.selected(), None);
    }

    #[test]
    fn update_input_reranks_candidates_by_haystack_and_resets_selection() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/user/files/firefox"), 'f', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.update_input(Value::raw(";c"));

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn feed_reranks_new_candidates_against_existing_input() {
        let mut state = LauncherState::default();

        state.update_input(Value::raw(";fpaper"));
        state.feed([
            Candidate::new(Value::raw("paperclip"), 'c', None),
            Candidate::new(Value::escaped("/home/user/files/paper.pdf"), 'f', None),
        ]);

        assert_eq!(
            state.selected(),
            Some(Value::escaped("/home/user/files/paper.pdf"))
        );
    }

    #[test]
    fn update_input_with_no_matches_clears_selection() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("zzz"));

        assert_eq!(state.selected(), None);
    }

    #[test]
    fn feed_appends_candidate_matches_from_multiple_sources() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("calculator"), 'c', None)]);
        state.feed([Candidate::new(
            Value::escaped("/home/user/files/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw(";c"));

        assert_eq!(state.selected(), Some(Value::raw("calculator")));
    }

    #[test]
    fn update_input_in_edit_mode_edits_value_and_ignores_results() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.press_tilde();
        state.update_input(Value::raw("f"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("f"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn update_input_in_search_mode_reranks_and_resets_selection() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/user/files/firefox"), 'f', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.select_next();
        assert_eq!(state.selected(), Some(Value::raw("firefox")));

        state.update_input(Value::raw(";f"));

        assert_eq!(
            state.selected(),
            Some(Value::escaped("/home/user/files/firefox"))
        );
    }

    #[test]
    fn update_input_in_edit_mode_does_not_rerank() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("firefox"), 'c', None),
            Candidate::new(Value::escaped("/home/me/firefox.pdf"), 'f', None),
        ]);
        state.press_tilde();

        state.update_input(Value::raw(";f firefox"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(";f firefox"));
        assert_eq!(state.current(), Value::raw(";f firefox"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn tilde_with_empty_input_ignores_selected_match() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn tilde_with_no_selected_match_keeps_typed_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        state.press_tilde();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("ps aux | grep firefox"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn left_brace_in_search_mode_enters_edit_mode_from_current_value() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c', None)]);
        state.update_input(Value::raw("mv "));

        state.update_input(Value::raw("mv {"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("mv {"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn left_brace_in_edit_mode_updates_input_without_search_resolution() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c', None)]);
        state.press_tilde();

        state.update_input(Value::raw("{"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("{"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn tab_composes_current_value_into_queue() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw(";f"));

        state.press_tab();

        assert_eq!(state.queue_status(), Some("'/home/me/paper.pdf'".into()));
    }

    #[test]
    fn tab_resets_launcher_state_and_restores_original_result_order() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("firefox"), 'c', None),
            Candidate::new(Value::escaped("/home/me/paper.pdf"), 'f', None),
        ]);
        state.update_input(Value::raw(";f"));

        state.press_tab();

        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.selected(), Some(Value::raw("firefox")));
        assert_eq!(state.queue_status(), Some("'/home/me/paper.pdf'".into()));
    }

    #[test]
    fn tab_keeps_candidates_available_after_reset() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/me/paper.pdf"), 'f', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.update_input(Value::raw(";f"));
        state.press_tab();

        state.update_input(Value::raw(";c"));

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn tab_with_empty_input_does_not_queue_empty_command() {
        let mut state = LauncherState::default();

        state.press_tab();

        assert_eq!(state.queue_status(), None);
        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn tab_with_command_slots_composes_from_queue() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw(";f"));
        state.press_tab();
        state.update_input(Value::raw("readlink -f {}"));

        state.press_tab();

        assert_eq!(
            state.queue_status(),
            Some("readlink -f '/home/me/paper.pdf'".into())
        );
    }

    #[test]
    fn enter_composes_current_value_and_compiles_queue() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw(";f"));
        state.press_tab();
        state.update_input(Value::raw("evince"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("evince '/home/me/paper.pdf'"))
        );
    }

    #[test]
    fn enter_resets_launcher_state_and_restores_original_result_order() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("evince"), 'c', None),
            Candidate::new(Value::escaped("/home/me/paper.pdf"), 'f', None),
        ]);
        state.update_input(Value::raw(";f"));
        state.press_tab();
        state.update_input(Value::raw("evince"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("evince '/home/me/paper.pdf'"))
        );

        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.selected(), Some(Value::raw("evince")));
        assert_eq!(
            state.queue_status(),
            Some("evince '/home/me/paper.pdf'".into())
        );
    }

    #[test]
    fn enter_keeps_candidates_available_after_reset() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("firefox"), 'c', None),
            Candidate::new(Value::raw("evince"), 'c', None),
        ]);
        state.update_input(Value::raw("evince"));
        assert_eq!(state.press_enter(), Some(Value::raw("evince")));

        state.update_input(Value::raw("fire"));

        assert_eq!(state.selected(), Some(Value::raw("firefox")));
    }

    #[test]
    fn enter_with_empty_input_returns_none_and_does_not_queue() {
        let mut state = LauncherState::default();

        assert_eq!(state.press_enter(), None);

        assert_eq!(state.queue_status(), None);
        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn enter_with_unfilled_slots_queues_incomplete_value() {
        let mut state = LauncherState::default();

        state.update_input(Value::raw("readlink -f {}"));

        assert_eq!(state.press_enter(), None);
        assert_eq!(state.queue_status(), Some("readlink -f {}".into()));
    }

    #[test]
    fn enter_with_unfilled_slots_behaves_like_tab() {
        let mut enter_state = LauncherState::default();
        enter_state.update_input(Value::raw("xdg-open {}"));

        let mut tab_state = LauncherState::default();
        tab_state.update_input(Value::raw("xdg-open {}"));

        assert_eq!(enter_state.press_enter(), None);
        tab_state.press_tab();

        assert_eq!(enter_state.queue_status(), tab_state.queue_status());
        assert_eq!(enter_state.mode(), tab_state.mode());
        assert_eq!(enter_state.value(), tab_state.value());
        assert_eq!(enter_state.selected(), tab_state.selected());
    }

    #[test]
    fn queue_status_is_exposed_by_state() {
        let mut state = LauncherState::default();

        assert_eq!(state.queue_status(), None);

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw(";f"));
        state.press_tab();

        assert_eq!(state.queue_status(), Some("'/home/me/paper.pdf'".into()));
    }

    #[test]
    fn enter_returns_execute_when_queue_compiles() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw(";f"));
        state.press_tab();
        state.update_input(Value::raw("xdg-open"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("xdg-open '/home/me/paper.pdf'"))
        );
    }

    #[test]
    fn enter_with_empty_queue_uses_selected_values_direct_action() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            Some(Value::raw("xdg-open {}")),
        )]);
        state.update_input(Value::raw(";f"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("xdg-open '/home/me/paper.pdf'"))
        );
    }

    #[test]
    fn enter_after_initial_tilde_executes_typed_raw_command() {
        let mut state = LauncherState::default();

        state.press_tilde();
        state.update_input(Value::raw("ps aux | grep firefox"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("ps aux | grep firefox"))
        );
    }

    #[test]
    fn tab_queues_typed_raw_command_extending_selected_prefix() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("firefox --private-window"));

        state.press_tab();

        assert_eq!(
            state.queue_status(),
            Some("firefox --private-window".into())
        );
    }

    #[test]
    fn left_brace_shortcut_preserves_slot_text() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c', None)]);
        state.update_input(Value::raw("mv "));
        state.update_input(Value::raw("mv {}"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("mv {}"));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn enter_with_no_matches_executes_typed_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("ps aux | grep firefox"))
        );
    }
}
