use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionPolicy {
    Raw,
    Escaped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub editable_text: String,
    pub insertion_policy: InsertionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    value: Value,
    selector: char,
    direct_action: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    values: VecDeque<Value>,
}

impl Queue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_values(values: impl IntoIterator<Item = Value>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn compose(&mut self, current: Value) {
        let current = self.compose_current(current);
        self.values.push_back(current);
    }

    pub fn status(&self) -> Option<String> {
        if self.values.is_empty() {
            return None;
        }

        Some(render_command_order(&self.values))
    }

    pub fn compile(&self) -> Option<Value> {
        if self.values.is_empty() || self.values.iter().any(Value::has_slots) {
            return None;
        }

        Some(Value::raw(render_command_order(&self.values)))
    }

    fn compose_current(&mut self, current: Value) -> Value {
        if current.has_slots() {
            return current.fill_slots_from_queue(&mut self.values);
        }

        if self.values.front().is_some_and(Value::has_slots) {
            let queued = self
                .values
                .pop_front()
                .expect("front checked before pop_front");

            let mut values = VecDeque::from([current]);
            return queued.fill_slots_from_queue(&mut values);
        }

        current
    }
}

fn render_command_order(values: &VecDeque<Value>) -> String {
    let Some(current) = values.back() else {
        return String::new();
    };

    let mut parts = Vec::with_capacity(values.len());
    parts.push(crate::shell::render_value(current));
    parts.extend(
        values
            .iter()
            .take(values.len() - 1)
            .map(crate::shell::render_value),
    );

    parts.join(" ")
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

    fn fill_slots_from_queue(self, values: &mut VecDeque<Value>) -> Self {
        let mut text = self.editable_text;

        while let Some(value) = values.pop_front() {
            let Some(slot_index) = text.find("{}") else {
                values.push_front(value);
                break;
            };

            let inserted = crate::shell::render_value(&value);
            text.replace_range(slot_index..slot_index + 2, &inserted);
        }

        Self {
            editable_text: text,
            insertion_policy: InsertionPolicy::Raw,
        }
    }
}

impl Candidate {
    pub fn new(value: Value, selector: char, direct_action: Option<Value>) -> Self {
        Self {
            value,
            selector,
            direct_action,
        }
    }

    pub(crate) fn value(&self) -> &Value {
        &self.value
    }

    pub(crate) fn direct_action(&self) -> Option<&Value> {
        self.direct_action.as_ref()
    }

    pub(crate) fn selector(&self) -> char {
        self.selector
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::render_value;

    fn compile_text(queue: &Queue) -> String {
        render_value(&queue.compile().expect("queue should compile"))
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

        assert_eq!(render_value(&command), "cmd a b");
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
