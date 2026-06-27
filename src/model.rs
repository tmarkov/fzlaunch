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
}
