pub mod directories;
pub mod executables;
pub mod files;

use crate::model::Value;

pub trait Source {
    fn values(&self, query: &str) -> Vec<Value>;
}
