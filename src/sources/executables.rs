use crate::model::Value;

pub fn executable_value(command: impl Into<String>) -> Value {
    Value::raw(command)
}
