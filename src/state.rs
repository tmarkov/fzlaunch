use std::cmp::Reverse;

use crate::history::edited_history_candidate;
use crate::model::{
    Action, Candidate, CandidateSource, ExecutionMode, ExecutionPlan, Queue, Value,
};
use crate::shell;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

const SORTED_RESULT_PREFIX_LEN: usize = 100;
const MATCH_SCORE_SCALE: u64 = 1_000;
const MAX_LENGTH_SCORE_BIAS: u64 = MATCH_SCORE_SCALE - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Search,
    Edit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRow {
    pub haystack: String,
    pub match_indices: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherState {
    mode: InputMode,
    value: Value,
    candidates: Vec<Candidate>,
    results: Vec<RankedCandidate>,
    selected_index: Option<usize>,
    queue: Queue,
    edit_origin: Option<Candidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RankedCandidate {
    index: usize,
    score: u64,
    row: ResultRow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchInput {
    needle: String,
    append: Vec<String>,
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
            edit_origin: None,
        }
    }
}

impl LauncherState {
    pub fn feed(&mut self, candidates: impl IntoIterator<Item = Candidate>) {
        let pattern = self.search_pattern();
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buf = Vec::new();
        let mut new_results = Vec::new();

        for candidate in candidates {
            let index = self.candidates.len();

            if self.mode == InputMode::Search {
                if let Some(result) =
                    rank_candidate(index, &candidate, pattern.as_ref(), &mut matcher, &mut buf)
                {
                    new_results.push(result);
                }
            }

            self.candidates.push(candidate);
        }

        if new_results.is_empty() {
            return;
        }

        new_results.sort_by_key(|result| Reverse(result.score));
        self.results = merge_ranked_results(std::mem::take(&mut self.results), new_results);
        if self.selected_index.is_none() && !self.results.is_empty() {
            self.selected_index = Some(0);
        }
    }

    pub fn replace_candidates_from_source(
        &mut self,
        source: CandidateSource,
        candidates: impl IntoIterator<Item = Candidate>,
    ) {
        self.candidates
            .retain(|candidate| candidate.source() != source);
        self.candidates.extend(candidates);

        if self.mode == InputMode::Search {
            self.rerank();
        }
    }

    pub fn replace_candidates_from_plugin(
        &mut self,
        source_id: &str,
        candidates: impl IntoIterator<Item = Candidate>,
    ) {
        self.candidates.retain(|candidate| {
            candidate.source() != CandidateSource::Plugin
                || candidate.source_id() != Some(source_id)
        });
        self.candidates.extend(candidates);

        if self.mode == InputMode::Search {
            self.rerank();
        }
    }

    pub fn update_input(&mut self, value: Value) {
        self.value = value;

        if self.mode == InputMode::Search {
            self.rerank();
        } else if self.value.editable_text().is_empty() {
            self.edit_origin = None;
        }
    }

    fn rerank(&mut self) {
        let pattern = self.search_pattern();
        let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
        let mut buf = Vec::new();
        self.results = self
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                rank_candidate(index, candidate, pattern.as_ref(), &mut matcher, &mut buf)
            })
            .collect();
        self.results.sort_by_key(|result| Reverse(result.score));

        self.selected_index = (!self.results.is_empty()).then_some(0);
    }

    fn search_pattern(&self) -> Option<Pattern> {
        let input = SearchInput::parse(self.value.editable_text());
        if input.needle.is_empty() {
            return None;
        }

        Some(Pattern::parse(
            &input.needle,
            CaseMatching::Ignore,
            Normalization::Smart,
        ))
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

    pub fn press_backtick(&mut self) {
        let edit_origin = if self.value.editable_text().is_empty() {
            None
        } else {
            self.selected()
        };
        let value = if self.value.editable_text().is_empty() {
            Value::raw("")
        } else {
            self.current()
        };

        self.mode = InputMode::Edit;
        self.value = value;
        self.results.clear();
        self.selected_index = None;
        self.edit_origin = edit_origin;
    }

    pub fn press_tab(&mut self) {
        let current = self.current_action();
        if !current.value().editable_text().is_empty() {
            self.queue.compose(current);
        }
        self.reset_input();
    }

    pub fn press_enter(&mut self) -> Option<ExecutionPlan> {
        let current = self.current_action();
        if current.value().editable_text().is_empty() {
            self.reset_input();
            return None;
        }

        if self.queue.is_empty() {
            if let Some(direct_action) = self
                .selected_index
                .and_then(|index| self.results.get(index))
                .and_then(|result| self.candidates.get(result.index))
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
        self.current_action().value().clone()
    }

    fn current_action(&self) -> Action {
        if self.mode == InputMode::Edit {
            return Action::foreground(self.value.clone());
        }

        let input = SearchInput::parse(self.value.editable_text());
        match self.selected_entry() {
            Some(candidate) => Action::new(
                selected_value_with_append_terms(candidate, &input.append),
                selected_value_execution_mode(candidate),
            ),
            None => Action::foreground(self.value.clone()),
        }
    }

    pub fn selected(&self) -> Option<Candidate> {
        self.selected_entry().cloned()
    }

    pub(crate) fn history_candidate(&self) -> Option<Candidate> {
        let current = self.current();
        if current.editable_text().is_empty() {
            return None;
        }

        if self.mode == InputMode::Edit {
            return self
                .edit_origin
                .as_ref()
                .filter(|origin| is_history_recordable(origin))
                .map(|origin| edited_history_candidate(current, origin));
        }

        match self.selected() {
            Some(selected) if !is_history_recordable(&selected) => None,
            Some(selected) if selected.value() == &current => Some(selected),
            Some(selected) => Some(edited_history_candidate(current, &selected)),
            None => None,
        }
    }

    pub fn results(&self) -> Vec<ResultRow> {
        self.results
            .iter()
            .map(|result| result.row.clone())
            .collect()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    fn reset_input(&mut self) {
        self.mode = InputMode::Search;
        self.value = Value::raw("");
        self.edit_origin = None;
        self.rerank();
    }

    fn selected_entry(&self) -> Option<&Candidate> {
        self.selected_index
            .and_then(|index| self.results.get(index))
            .and_then(|result| self.candidates.get(result.index))
    }
}

fn preference_score_adjustment(candidate: &Candidate) -> u64 {
    candidate.preference_score_millis() as u64
}

impl SearchInput {
    fn parse(input: &str) -> Self {
        let mut needle = Vec::new();
        let mut append = Vec::new();

        for term in input.split_whitespace() {
            if is_append_term(term) {
                append.push(term.to_string());
            } else {
                needle.push(term);
            }
        }

        Self {
            needle: needle.join(" "),
            append,
        }
    }
}

fn is_append_term(term: &str) -> bool {
    term == "{}" || term == "'{}'" || term.starts_with('-')
}

fn append_search_terms(value: Value, append: &[String]) -> Value {
    if append.is_empty() {
        return value;
    }

    Value::raw(format!(
        "{} {}",
        shell::render_value(&value),
        append.join(" ")
    ))
}

fn selected_value_with_append_terms(candidate: &Candidate, append: &[String]) -> Value {
    if candidate.source() == CandidateSource::Calculator {
        return candidate.value().clone();
    }

    append_search_terms(candidate.value().clone(), append)
}

fn is_history_recordable(candidate: &Candidate) -> bool {
    candidate.source() != CandidateSource::Calculator
}

fn selected_value_execution_mode(candidate: &Candidate) -> ExecutionMode {
    if candidate.source() == CandidateSource::FilesystemPath {
        return ExecutionMode::Foreground;
    }

    candidate
        .direct_action()
        .map(Action::execution_mode)
        .unwrap_or(ExecutionMode::Foreground)
}

fn rank_candidate(
    index: usize,
    candidate: &Candidate,
    pattern: Option<&Pattern>,
    matcher: &mut Matcher,
    buf: &mut Vec<char>,
) -> Option<RankedCandidate> {
    let (score, match_indices) = match pattern {
        Some(pattern) => {
            let mut indices = Vec::new();
            let score = pattern.indices(
                Utf32Str::new(candidate.haystack(), buf),
                matcher,
                &mut indices,
            )? as u64
                * MATCH_SCORE_SCALE
                + preference_score_adjustment(candidate)
                + length_score_adjustment(candidate);
            indices.sort_unstable();
            indices.dedup();
            (
                score,
                indices.into_iter().map(|index| index as usize).collect(),
            )
        }
        None => (preference_score_adjustment(candidate), Vec::new()),
    };

    Some(RankedCandidate {
        index,
        score,
        row: ResultRow {
            haystack: candidate.haystack().to_string(),
            match_indices,
        },
    })
}

fn length_score_adjustment(candidate: &Candidate) -> u64 {
    MAX_LENGTH_SCORE_BIAS.saturating_sub(candidate.value().editable_text().chars().count() as u64)
}

fn merge_ranked_results(
    existing: Vec<RankedCandidate>,
    incoming: Vec<RankedCandidate>,
) -> Vec<RankedCandidate> {
    let prefix_len = existing.len().min(SORTED_RESULT_PREFIX_LEN);
    let mut existing = existing.into_iter();
    let existing_prefix = existing.by_ref().take(prefix_len).collect::<Vec<_>>();
    let existing_tail = existing.collect::<Vec<_>>();
    let mut existing_prefix = existing_prefix.into_iter().peekable();
    let mut incoming = incoming.into_iter().peekable();
    let mut merged = Vec::new();

    while merged.len() < SORTED_RESULT_PREFIX_LEN {
        match (existing_prefix.peek(), incoming.peek()) {
            (Some(existing), Some(new)) if existing.score >= new.score => {
                merged.push(existing_prefix.next().expect("peeked existing result"));
            }
            (Some(_), Some(_)) => {
                merged.push(incoming.next().expect("peeked incoming result"));
            }
            (Some(_), None) => {
                merged.push(existing_prefix.next().expect("peeked existing result"));
            }
            (None, Some(_)) => {
                merged.push(incoming.next().expect("peeked incoming result"));
            }
            (None, None) => break,
        }
    }

    merged.extend(existing_prefix);
    merged.extend(incoming);
    merged.extend(existing_tail);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, ExecutionMode};

    fn selected_value(state: &LauncherState) -> Option<Value> {
        state.selected().map(|candidate| candidate.value().clone())
    }

    #[test]
    fn initial_backtick_enters_edit_mode_with_empty_raw_buffer() {
        let mut state = LauncherState::default();

        state.press_backtick();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(""));
    }

    #[test]
    fn backtick_with_search_input_seeds_edit_mode_from_selected_match() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/me/Documents/research"), 'd', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.update_input(Value::raw(";d"));

        state.press_backtick();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::escaped("/home/me/Documents/research"));
        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn backtick_with_append_terms_seeds_edit_mode_from_selected_match_plus_append_terms() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("gnome-terminal"), 'c', None)]);
        state.update_input(Value::raw("gterm -c {}"));

        state.press_backtick();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("gnome-terminal -c {}"));
        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn search_input_without_prefix_resolves_to_selected_match() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("fir"));

        assert_eq!(state.current(), Value::raw("firefox"));
    }

    #[test]
    fn search_input_with_append_term_resolves_to_selected_match_plus_append_term() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("firefox --private-window"));

        assert_eq!(state.current(), Value::raw("firefox --private-window"));
    }

    #[test]
    fn search_input_with_arbitrary_extra_text_resolves_to_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("firefox private-window"));

        assert_eq!(state.current(), Value::raw("firefox private-window"));
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
        state.press_backtick();
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

        assert_eq!(selected_value(&state), Some(Value::raw("firefox")));
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
                ResultRow {
                    haystack: ";c/firefox".to_string(),
                    match_indices: Vec::new(),
                },
                ResultRow {
                    haystack: ";f//home/me/paper.pdf".to_string(),
                    match_indices: Vec::new(),
                }
            ]
        );
    }

    #[test]
    fn results_include_matched_haystack_indices() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw("paper"));

        assert_eq!(
            state.results(),
            vec![ResultRow {
                haystack: ";f//home/me/paper.pdf".to_string(),
                match_indices: vec![12, 13, 14, 15, 16],
            }]
        );
    }

    #[test]
    fn search_uses_candidate_haystack_but_current_uses_value() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/me/project/src/cache.rs"), 'r', None)
                .with_haystack(";r src/cache.rs:45 fixed memory leak in eviction path"),
        ]);
        state.update_input(Value::raw("memory leak ;r"));

        assert_eq!(
            selected_value(&state),
            Some(Value::escaped("/home/me/project/src/cache.rs"))
        );
        let results = state.results();
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].haystack,
            ";r src/cache.rs:45 fixed memory leak in eviction path"
        );
    }

    #[test]
    fn direct_action_composes_selected_value_not_haystack() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::raw("7"),
            '=',
            Some(Value::raw("printf %s {} | wl-copy")),
        )
        .with_haystack(";= 3 + 4 = 7")]);
        state.update_input(Value::raw("3 + 4 ;="));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("printf %s 7 | wl-copy").into())
        );
    }

    #[test]
    fn calculator_choices_are_not_history_candidates() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::raw("7"),
            '=',
            Some(Value::raw("printf %s {} | wl-copy")),
        )
        .with_source(CandidateSource::Calculator)
        .with_haystack(";= 3 + 4 = 7")]);
        state.update_input(Value::raw("3 + 4 ;="));

        assert_eq!(state.history_candidate(), None);

        state.press_backtick();
        state.update_input(Value::raw("8"));

        assert_eq!(state.history_candidate(), None);
    }

    #[test]
    fn feeding_new_candidates_preserves_selection_index() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c', None),
            Candidate::new(Value::raw("second"), 'c', None),
        ]);
        state.select_next();
        assert_eq!(selected_value(&state), Some(Value::raw("second")));

        state.feed([
            Candidate::new(Value::raw("new-first"), 'c', None).with_preference_score(10),
            Candidate::new(Value::raw("new-second"), 'c', None),
        ]);

        assert_eq!(state.selected_index(), Some(1));
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

        assert_eq!(selected_value(&state), Some(Value::raw("first")));
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

        assert_eq!(selected_value(&state), Some(Value::raw("second")));
    }

    #[test]
    fn selection_stays_at_first_match_when_moving_up_past_start() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("first"), 'c', None),
            Candidate::new(Value::raw("second"), 'c', None),
        ]);
        state.select_previous();

        assert_eq!(selected_value(&state), Some(Value::raw("first")));
    }

    #[test]
    fn select_next_with_no_results_is_noop() {
        let mut state = LauncherState::default();

        state.select_next();

        assert_eq!(selected_value(&state), None);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.mode(), InputMode::Search);
    }

    #[test]
    fn select_previous_with_no_results_is_noop() {
        let mut state = LauncherState::default();

        state.select_previous();

        assert_eq!(selected_value(&state), None);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(state.mode(), InputMode::Search);
    }

    #[test]
    fn feed_empty_candidates_keeps_selection_empty_without_existing_candidates() {
        let mut state = LauncherState::default();

        state.feed([]);

        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn update_input_reranks_candidates_by_haystack_and_resets_selection() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/user/files/firefox"), 'f', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.update_input(Value::raw(";c"));

        assert_eq!(selected_value(&state), Some(Value::raw("firefox")));
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
            selected_value(&state),
            Some(Value::escaped("/home/user/files/paper.pdf"))
        );
    }

    #[test]
    fn feed_can_promote_new_matches_after_many_existing_matches() {
        let mut state = LauncherState::default();

        state.update_input(Value::raw(";c item"));
        state.feed(
            (0..150).map(|index| Candidate::new(Value::raw(format!("item-{index:03}")), 'c', None)),
        );
        state.feed([Candidate::new(Value::raw("item-999"), 'c', None).with_preference_score(10)]);

        assert_eq!(selected_value(&state), Some(Value::raw("item-999")));
    }

    #[test]
    fn shorter_candidate_wins_when_match_scores_are_equal() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("cats"), 'c', None),
            Candidate::new(Value::raw("cat"), 'c', None),
        ]);
        state.update_input(Value::raw("cat"));

        assert_eq!(selected_value(&state), Some(Value::raw("cat")));
    }

    #[test]
    fn exact_command_segment_wins_over_longer_path_segment() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("tmp/cat"), 'c', None),
            Candidate::new(Value::raw("cat"), 'c', None),
        ]);
        state.update_input(Value::raw("cat"));

        assert_eq!(selected_value(&state), Some(Value::raw("cat")));
    }

    #[test]
    fn results_use_path_delimiter_for_matching_haystack() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("cat"), 'c', None)]);
        state.update_input(Value::raw("cat"));

        assert_eq!(
            state.results(),
            vec![ResultRow {
                haystack: ";c/cat".to_string(),
                match_indices: vec![3, 4, 5],
            }]
        );
    }

    #[test]
    fn empty_input_keeps_unpreferred_candidates_in_source_order() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("much-longer"), 'c', None),
            Candidate::new(Value::raw("a"), 'c', None),
        ]);

        assert_eq!(selected_value(&state), Some(Value::raw("much-longer")));
    }

    #[test]
    fn preference_score_breaks_fuzzy_score_ties() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("bar"), 'c', None),
            Candidate::new(Value::raw("baz"), 'c', None),
            Candidate::new(Value::raw("bash"), 'c', None).with_preference_score(10),
        ]);
        state.update_input(Value::raw("ba"));

        assert_eq!(selected_value(&state), Some(Value::raw("bash")));
    }

    #[test]
    fn preference_score_can_overcome_small_fuzzy_score_differences() {
        let mut state = LauncherState::default();
        let noisy_path = "/home/todor/dev/fzlaunch/target/debug/incremental/fzlaunch-0r0jvnbum6sdj/s-hk317rysej-1nuonkh-evg95m50mdruu5xt7489zborz/baaoui0l7egf4cmlwezbrha5f.o";

        state.feed([
            Candidate::new(Value::escaped(noisy_path), 'f', None),
            Candidate::new(Value::raw("bash"), 'c', None).with_preference_score(10),
        ]);
        state.update_input(Value::raw("ba"));

        assert_eq!(selected_value(&state), Some(Value::raw("bash")));
    }

    #[test]
    fn later_high_preference_candidates_are_inserted_into_base_order() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::raw("bar"), 'c', None),
            Candidate::new(Value::raw("baz"), 'c', None),
        ]);
        state.feed([Candidate::new(Value::raw("bash"), 'c', None).with_preference_score(10)]);

        assert_eq!(selected_value(&state), Some(Value::raw("bash")));
    }

    #[test]
    fn update_input_with_no_matches_clears_selection() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("zzz"));

        assert_eq!(selected_value(&state), None);
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

        assert_eq!(selected_value(&state), Some(Value::raw("calculator")));
    }

    #[test]
    fn feeding_unpreferred_candidates_with_empty_input_appends_results() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("first"), 'c', None)]);
        state.feed([Candidate::new(Value::raw("second"), 'c', None)]);

        assert_eq!(
            state.results(),
            vec![
                ResultRow {
                    haystack: ";c/first".to_string(),
                    match_indices: Vec::new(),
                },
                ResultRow {
                    haystack: ";c/second".to_string(),
                    match_indices: Vec::new(),
                },
            ]
        );
        assert_eq!(selected_value(&state), Some(Value::raw("first")));
    }

    #[test]
    fn update_input_in_edit_mode_edits_value_and_ignores_results() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.press_backtick();
        state.update_input(Value::raw("f"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("f"));
        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn update_input_in_search_mode_reranks_and_resets_selection() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/user/files/firefox"), 'f', None),
            Candidate::new(Value::raw("firefox"), 'c', None),
        ]);
        state.select_next();
        assert_eq!(selected_value(&state), Some(Value::raw("firefox")));

        state.update_input(Value::raw(";f"));

        assert_eq!(
            selected_value(&state),
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
        state.press_backtick();

        state.update_input(Value::raw(";f firefox"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(";f firefox"));
        assert_eq!(state.current(), Value::raw(";f firefox"));
        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn backtick_with_empty_input_ignores_selected_match() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.press_backtick();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn backtick_with_no_selected_match_keeps_typed_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        state.press_backtick();

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("ps aux | grep firefox"));
        assert_eq!(selected_value(&state), None);
    }

    #[test]
    fn left_brace_in_search_mode_updates_search_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c', None)]);
        state.update_input(Value::raw("mv "));

        state.update_input(Value::raw("mv {"));

        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw("mv {"));
        assert_eq!(state.current(), Value::raw("mv {"));
    }

    #[test]
    fn left_brace_in_edit_mode_updates_input_without_search_resolution() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c', None)]);
        state.press_backtick();

        state.update_input(Value::raw("{"));

        assert_eq!(state.mode(), InputMode::Edit);
        assert_eq!(state.value(), Value::raw("{"));
        assert_eq!(selected_value(&state), None);
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
        assert_eq!(selected_value(&state), Some(Value::raw("firefox")));
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

        assert_eq!(selected_value(&state), Some(Value::raw("firefox")));
    }

    #[test]
    fn tab_with_empty_input_does_not_queue_empty_command() {
        let mut state = LauncherState::default();

        state.press_tab();

        assert_eq!(state.queue_status(), None);
        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(selected_value(&state), None);
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
            Some(Value::raw("evince '/home/me/paper.pdf'").into())
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
            Some(Value::raw("evince '/home/me/paper.pdf'").into())
        );

        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(selected_value(&state), Some(Value::raw("evince")));
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
        assert_eq!(state.press_enter(), Some(Value::raw("evince").into()));

        state.update_input(Value::raw("fire"));

        assert_eq!(selected_value(&state), Some(Value::raw("firefox")));
    }

    #[test]
    fn enter_with_empty_input_returns_none_and_does_not_queue() {
        let mut state = LauncherState::default();

        assert_eq!(state.press_enter(), None);

        assert_eq!(state.queue_status(), None);
        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw(""));
        assert_eq!(selected_value(&state), None);
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
        assert_eq!(selected_value(&enter_state), selected_value(&tab_state));
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
            Some(Value::raw("xdg-open '/home/me/paper.pdf'").into())
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
            Some(Value::raw("xdg-open '/home/me/paper.pdf'").into())
        );
    }

    #[test]
    fn enter_uses_selected_direct_action_execution_mode() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new_with_action(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            Some(Action::detached(Value::raw("xdg-open {}"))),
        )]);
        state.update_input(Value::raw(";f"));

        assert_eq!(
            state
                .press_enter()
                .expect("command should compile")
                .execution_mode(),
            ExecutionMode::Detached
        );
    }

    #[test]
    fn composed_command_inherits_execution_mode_from_slot_template() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new_with_action(
            Value::raw("xdg-open {}"),
            'c',
            Some(Action::detached(Value::raw("{}"))),
        )]);
        state.update_input(Value::raw(";cxdg"));
        state.press_tab();
        state.update_input(Value::raw("/home/me/paper.pdf"));

        assert_eq!(
            state
                .press_enter()
                .expect("command should compile")
                .execution_mode(),
            ExecutionMode::Detached
        );
    }

    #[test]
    fn enter_after_initial_backtick_executes_typed_raw_command() {
        let mut state = LauncherState::default();

        state.press_backtick();
        state.update_input(Value::raw("ps aux | grep firefox"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("ps aux | grep firefox").into())
        );
    }

    #[test]
    fn tab_queues_selected_match_with_dash_append_term() {
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
    fn tab_queues_selected_match_with_slot_append_term() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("mv"), 'c', None)]);
        state.update_input(Value::raw("mv "));
        state.update_input(Value::raw("mv {}"));

        assert_eq!(state.mode(), InputMode::Search);
        assert_eq!(state.value(), Value::raw("mv {}"));
        state.press_tab();

        assert_eq!(state.queue_status(), Some("mv {}".into()));
    }

    #[test]
    fn tab_queues_selected_match_with_quoted_slot_append_term() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("printf"), 'c', None)]);
        state.update_input(Value::raw("printf '{}'"));

        state.press_tab();

        assert_eq!(state.queue_status(), Some("printf '{}'".into()));
    }

    #[test]
    fn append_terms_are_not_part_of_the_search_needle() {
        let mut state = LauncherState::default();

        state.feed([
            Candidate::new(Value::escaped("/home/me/mv {} notes.txt"), 'f', None),
            Candidate::new(Value::raw("mv"), 'c', None),
        ]);
        state.update_input(Value::raw("mv -i {}"));

        assert_eq!(selected_value(&state), Some(Value::raw("mv")));
        assert_eq!(state.value(), Value::raw("mv -i {}"));
    }

    #[test]
    fn append_terms_are_appended_to_fuzzy_selected_match() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("gnome-terminal"), 'c', None)]);
        state.update_input(Value::raw("gterm -c {}"));

        state.press_tab();

        assert_eq!(state.queue_status(), Some("gnome-terminal -c {}".into()));
    }

    #[test]
    fn append_terms_are_appended_to_escaped_selected_match() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(
            Value::escaped("/home/me/paper.pdf"),
            'f',
            None,
        )]);
        state.update_input(Value::raw("paper -v"));

        state.press_tab();

        assert_eq!(state.queue_status(), Some("'/home/me/paper.pdf' -v".into()));
    }

    #[test]
    fn enter_with_no_matches_executes_typed_raw_input() {
        let mut state = LauncherState::default();

        state.feed([Candidate::new(Value::raw("firefox"), 'c', None)]);
        state.update_input(Value::raw("ps aux | grep firefox"));

        assert_eq!(
            state.press_enter(),
            Some(Value::raw("ps aux | grep firefox").into())
        );
    }
}
