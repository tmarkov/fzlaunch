use crate::model::{DirectAction, Value};

pub fn executable_value(command: impl Into<String>) -> Value {
    Value {
        editable_text: command.into(),
        insertion_policy: crate::model::InsertionPolicy::Raw,
        direct_action: Some(DirectAction::Execute),
    }
}
