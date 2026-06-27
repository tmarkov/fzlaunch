use crate::model::Value;

pub fn file_value(path: impl Into<String>) -> Value {
    Value::escaped(path)
}
