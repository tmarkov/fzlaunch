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

    pub fn front(&self) -> Option<&Value> {
        self.values.front()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn compose_for_queue(&mut self, current: Value) {
        let current = self.compose_current(current);
        self.values.push_back(current);
    }

    pub fn compose_for_execute(&mut self, current: Value) -> Value {
        let current = self.compose_current(current);

        if self.values.is_empty() {
            return current;
        }

        let mut command = crate::shell::render_value(&current);

        while let Some(value) = self.values.pop_front() {
            command.push(' ');
            command.push_str(&crate::shell::render_value(&value));
        }

        Value::raw(command)
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

        queue.compose_for_queue(command);
        let composed = queue.front().expect("composed value should be queued");

        assert_eq!(composed.insertion_policy, InsertionPolicy::Raw);
        assert_eq!(
            render_value(&composed),
            "readlink -f '/home/me/link to paper.pdf'"
        );
    }

    #[test]
    fn execute_fills_slots_then_appends_remaining_queue_values_as_arguments() {
        let mut queue = Queue::from_values([Value::raw("a"), Value::raw("b")]);

        let command = queue.compose_for_execute(Value::raw("cmd {}"));

        assert_eq!(render_value(&command), "cmd a b");
        assert!(queue.is_empty());
    }
}
