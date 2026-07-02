use crate::model::{InsertionPolicy, Value};

pub fn render_value(value: &Value) -> String {
    match value.insertion_policy() {
        InsertionPolicy::Raw => value.editable_text().to_string(),
        InsertionPolicy::Escaped => quote_posix(value.editable_text()),
    }
}

pub fn quote_posix(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_with_single_quotes() {
        assert_eq!(quote_posix("/home/me/a'b.txt"), "'/home/me/a'\\''b.txt'");
    }
}
