use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "PascalCase")]
pub struct CodeFoldRegion {
    pub start_index: usize,
    pub end_index: usize,
    pub function_name: String,
}
