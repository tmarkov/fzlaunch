use crate::model::{DirectAction, Value};

pub fn directory_value(path: impl Into<String>) -> Value {
    Value {
        editable_text: path.into(),
        insertion_policy: crate::model::InsertionPolicy::Escaped,
        direct_action: Some(DirectAction::Open),
    }
}
