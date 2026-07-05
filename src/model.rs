use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionPolicy {
    Raw,
    Escaped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    editable_text: String,
    insertion_policy: InsertionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionMode {
    Foreground,
    Detached,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    value: Value,
    execution_mode: ExecutionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionPlan {
    command: Value,
    execution_mode: ExecutionMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    value: Value,
    selector: char,
    direct_action: Option<Action>,
    source: CandidateSource,
    preference_score_millis: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateSource {
    Generic,
    PathExecutable,
    FilesystemPath,
    History,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    values: VecDeque<Action>,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_values(values: impl IntoIterator<Item = Value>) -> Self {
        Self {
            values: values.into_iter().map(Action::foreground).collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn compose(&mut self, current: impl Into<Action>) {
        let current = self.compose_current(current.into());
        self.values.push_back(current);
    }

    pub fn status(&self) -> Option<String> {
        if self.values.is_empty() {
            return None;
        }

        Some(render_command_order(&self.values))
    }

    pub fn compile(&self) -> Option<ExecutionPlan> {
        let current = self.values.back()?;
        if self.values.iter().any(|action| action.value().has_slots()) {
            return None;
        }

        Some(ExecutionPlan {
            command: Value::raw(render_command_order(&self.values)),
            execution_mode: current.execution_mode(),
        })
    }

    fn compose_current(&mut self, current: Action) -> Action {
        if current.value().has_slots() {
            return current.fill_slots_from_queue(&mut self.values);
        }

        if let Some(queued) = self
            .values
            .pop_front_if(|action| action.value().has_slots())
        {
            let mut values = VecDeque::from([current]);
            return queued.fill_slots_from_queue(&mut values);
        }

        current
    }
}

fn render_command_order(values: &VecDeque<Action>) -> String {
    let Some(current) = values.back() else {
        return String::new();
    };

    let mut parts = Vec::with_capacity(values.len());
    parts.push(crate::shell::render_value(current.value()));
    parts.extend(
        values
            .iter()
            .take(values.len() - 1)
            .map(|action| crate::shell::render_value(action.value())),
    );

    parts.join(" ")
}

impl Action {
    pub fn foreground(value: Value) -> Self {
        Self {
            value,
            execution_mode: ExecutionMode::Foreground,
        }
    }

    pub fn detached(value: Value) -> Self {
        Self {
            value,
            execution_mode: ExecutionMode::Detached,
        }
    }

    pub fn new(value: Value, execution_mode: ExecutionMode) -> Self {
        Self {
            value,
            execution_mode,
        }
    }

    pub fn value(&self) -> &Value {
        &self.value
    }

    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    fn fill_slots_from_queue(self, values: &mut VecDeque<Action>) -> Self {
        Self {
            value: self.value.fill_slots_from_queue(values),
            execution_mode: self.execution_mode,
        }
    }
}

impl From<Value> for Action {
    fn from(value: Value) -> Self {
        Self::foreground(value)
    }
}

impl ExecutionPlan {
    pub fn new(command: Value, execution_mode: ExecutionMode) -> Self {
        Self {
            command,
            execution_mode,
        }
    }

    pub fn command(&self) -> &Value {
        &self.command
    }

    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }
}

impl PartialEq<Value> for ExecutionPlan {
    fn eq(&self, other: &Value) -> bool {
        &self.command == other
    }
}

impl PartialEq<ExecutionPlan> for Value {
    fn eq(&self, other: &ExecutionPlan) -> bool {
        self == &other.command
    }
}

impl From<Value> for ExecutionPlan {
    fn from(command: Value) -> Self {
        Self::new(command, ExecutionMode::Foreground)
    }
}

impl Value {
    pub fn raw(editable_text: impl Into<String>) -> Self {
        Self {
            editable_text: editable_text.into(),
            insertion_policy: InsertionPolicy::Raw,
        }
    }

    pub fn escaped(editable_text: impl Into<String>) -> Self {
        Self {
            editable_text: editable_text.into(),
            insertion_policy: InsertionPolicy::Escaped,
        }
    }

    pub fn has_slots(&self) -> bool {
        self.editable_text.contains("{}")
    }

    pub fn editable_text(&self) -> &str {
        &self.editable_text
    }

    pub fn insertion_policy(&self) -> InsertionPolicy {
        self.insertion_policy
    }

    pub fn edit_text(&mut self, update: impl FnOnce(&mut String)) {
        update(&mut self.editable_text);
    }

    fn fill_slots_from_queue(self, values: &mut VecDeque<Action>) -> Self {
        let mut text = self.editable_text;

        while let Some(value) = values.pop_front() {
            let Some(slot_index) = text.find("{}") else {
                values.push_front(value);
                break;
            };

            let inserted = crate::shell::render_value(value.value());
            text.replace_range(slot_index..slot_index + 2, &inserted);
        }

        Self {
            editable_text: text,
            insertion_policy: InsertionPolicy::Raw,
        }
    }
}

impl Candidate {
    #[cfg(test)]
    pub fn new(value: Value, selector: char, direct_action: Option<Value>) -> Self {
        Self::new_with_action(value, selector, direct_action.map(Action::foreground))
    }

    pub fn new_with_action(value: Value, selector: char, direct_action: Option<Action>) -> Self {
        Self {
            value,
            selector,
            direct_action,
            source: CandidateSource::Generic,
            preference_score_millis: 0,
        }
    }

    pub fn with_source(mut self, source: CandidateSource) -> Self {
        self.source = source;
        self
    }

    #[cfg(test)]
    pub fn with_preference_score(mut self, preference_score: u32) -> Self {
        self.preference_score_millis = preference_score.saturating_mul(1_000);
        self
    }

    pub(crate) fn with_preference_score_millis(mut self, preference_score_millis: u32) -> Self {
        self.preference_score_millis = preference_score_millis;
        self
    }

    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    pub(crate) fn direct_action(&self) -> Option<&Action> {
        self.direct_action.as_ref()
    }

    pub(crate) fn selector(&self) -> char {
        self.selector
    }

    pub(crate) fn source(&self) -> CandidateSource {
        self.source
    }

    pub(crate) fn preference_score_millis(&self) -> u32 {
        self.preference_score_millis
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::render_value;

    fn compile_text(queue: &Queue) -> String {
        render_value(queue.compile().expect("queue should compile").command())
    }

    #[test]
    fn fills_slot_with_escaped_value_and_produces_raw_shell_fragment() {
        let file = Value::escaped("/home/me/link to paper.pdf");
        let command = Value::raw("readlink -f {}");
        let mut queue = Queue::from_values([file]);

        queue.compose(command);

        assert_eq!(
            queue.status(),
            Some("readlink -f '/home/me/link to paper.pdf'".to_string())
        );
    }

    #[test]
    fn execute_fills_slots_then_appends_remaining_queue_values_as_arguments() {
        let mut queue = Queue::from_values([Value::raw("a"), Value::raw("b")]);

        queue.compose(Value::raw("cmd {}"));
        let command = queue.compile().expect("queue should compile");

        assert_eq!(render_value(command.command()), "cmd a b");
    }

    #[test]
    fn composed_command_inherits_execution_mode_from_slot_template() {
        let mut queue = Queue::from_values([Value::escaped("/home/me/paper.pdf")]);

        queue.compose(Action::detached(Value::raw("xdg-open {}")));

        assert_eq!(
            queue
                .compile()
                .expect("queue should compile")
                .execution_mode(),
            ExecutionMode::Detached
        );
    }

    #[test]
    fn compile_returns_none_if_queue_has_unfilled_slots() {
        let mut queue = Queue::new();

        queue.compose(Value::raw("readlink -f {}"));

        assert_eq!(queue.compile(), None);
    }

    #[test]
    fn compile_returns_none_if_queue_is_empty() {
        let queue = Queue::new();

        assert_eq!(queue.compile(), None);
    }

    #[test]
    fn status_renders_current_command_before_queued_arguments() {
        let mut queue = Queue::new();

        queue.compose(Value::escaped(
            "/home/me/Documents/research/2024-polynomial-interpolation.pdf",
        ));
        queue.compose(Value::raw("evince"));

        assert_eq!(
            queue.status(),
            Some("evince '/home/me/Documents/research/2024-polynomial-interpolation.pdf'".into())
        );
    }

    #[test]
    fn compiles_file_argument_with_chosen_program() {
        let mut queue = Queue::new();

        queue.compose(Value::escaped(
            "/home/me/Documents/research/2024-polynomial-interpolation.pdf",
        ));
        queue.compose(Value::raw("evince"));

        assert_eq!(
            compile_text(&queue),
            "evince '/home/me/Documents/research/2024-polynomial-interpolation.pdf'"
        );
    }

    #[test]
    fn composes_move_and_rename_with_two_slots() {
        let mut queue = Queue::new();

        queue.compose(Value::escaped("/home/me/Downloads/2024-8234.pdf"));
        queue.compose(Value::raw("securemove {} {}"));

        assert_eq!(
            queue.status(),
            Some("securemove '/home/me/Downloads/2024-8234.pdf' {}".into())
        );

        queue.compose(Value::escaped(
            "/home/me/Documents/research/2024-polynomial-interpolation.pdf",
        ));

        assert_eq!(
            compile_text(&queue),
            "securemove '/home/me/Downloads/2024-8234.pdf' '/home/me/Documents/research/2024-polynomial-interpolation.pdf'"
        );
    }

    #[test]
    fn current_value_with_multiple_slots_consumes_queued_values_fifo() {
        let mut queue = Queue::new();

        queue.compose(Value::raw("src"));
        queue.compose(Value::raw("dest"));
        queue.compose(Value::raw("mv {} {}"));

        assert_eq!(compile_text(&queue), "mv src dest");
    }

    #[test]
    fn composes_nested_shell_fragments() {
        let mut queue = Queue::new();

        queue.compose(Value::escaped("/home/me/link to paper.pdf"));
        queue.compose(Value::raw("readlink -f {}"));
        queue.compose(Value::raw("nvim $({})"));

        assert_eq!(
            compile_text(&queue),
            "nvim $(readlink -f '/home/me/link to paper.pdf')"
        );
    }

    #[test]
    fn preserves_slots_through_composition_until_later_fill() {
        let mut queue = Queue::new();

        queue.compose(Value::raw("readlink -f {}"));
        queue.compose(Value::raw("nvim $({})"));

        assert_eq!(queue.status(), Some("nvim $(readlink -f {})".into()));
        assert_eq!(queue.compile(), None);

        queue.compose(Value::escaped("/home/me/link to paper.pdf"));

        assert_eq!(
            compile_text(&queue),
            "nvim $(readlink -f '/home/me/link to paper.pdf')"
        );
    }

    #[test]
    fn brackets_slotted_value() {
        let mut queue = Queue::new();

        queue.compose(Value::raw("bar"));
        queue.compose(Value::raw("echo {{}}"));

        assert_eq!(compile_text(&queue), "echo {bar}");
    }

    #[test]
    fn escaped_value_with_single_quote_is_quoted_when_slotted() {
        let mut queue = Queue::new();

        queue.compose(Value::escaped("/home/me/a'b.txt"));
        queue.compose(Value::raw("cat {}"));

        assert_eq!(compile_text(&queue), "cat '/home/me/a'\\''b.txt'");
    }

    #[test]
    fn empty_escaped_value_is_rendered_as_empty_shell_string() {
        let mut queue = Queue::new();

        queue.compose(Value::escaped(""));
        queue.compose(Value::raw("printf %s {}"));

        assert_eq!(compile_text(&queue), "printf %s ''");
    }

    #[test]
    fn non_slot_braces_are_literal() {
        let mut queue = Queue::new();

        queue.compose(Value::raw("echo {file}"));

        assert_eq!(compile_text(&queue), "echo {file}");
    }
}
