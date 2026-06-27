use crate::model::Value;

pub fn directory_value(path: impl Into<String>) -> Value {
    Value::escaped(path)
}
