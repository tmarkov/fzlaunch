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
    pub direct_action: Option<DirectAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectAction {
    Open,
    Execute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileError {
    Empty,
    UnfilledSlots,
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

        Some(
            self.values
                .iter()
                .map(crate::shell::render_value)
                .collect::<Vec<_>>()
                .join(" "),
        )
    }

    pub fn compile(&self) -> Result<Value, CompileError> {
        let Some(current) = self.values.back() else {
            return Err(CompileError::Empty);
        };

        if self.values.iter().any(Value::has_slots) {
            return Err(CompileError::UnfilledSlots);
        }

        let mut parts = Vec::with_capacity(self.values.len());
        parts.push(crate::shell::render_value(current));
        parts.extend(
            self.values
                .iter()
                .take(self.values.len() - 1)
                .map(crate::shell::render_value),
        );

        Ok(Value::raw(parts.join(" ")))
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

impl Value {
    pub fn raw(editable_text: impl Into<String>) -> Self {
        Self {
            editable_text: editable_text.into(),
            insertion_policy: InsertionPolicy::Raw,
            direct_action: None,
        }
    }

    pub fn escaped(editable_text: impl Into<String>) -> Self {
        Self {
            editable_text: editable_text.into(),
            insertion_policy: InsertionPolicy::Escaped,
            direct_action: None,
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
            direct_action: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::render_value;

    #[test]
    fn fills_slot_with_escaped_value_and_produces_raw_shell_fragment() {
        let file = Value::escaped("/home/me/link to paper.pdf");
        let command = Value::raw("readlink -f {}");
        let mut queue = Queue::from_values([file]);

        assert_eq!(
            queue.status(),
            Some("'/home/me/link to paper.pdf'".to_string())
        );

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
    fn compile_fails_if_queue_has_unfilled_slots() {
        let mut queue = Queue::new();

        queue.compose(Value::raw("readlink -f {}"));

        assert_eq!(queue.compile(), Err(CompileError::UnfilledSlots));
    }
}
