pub mod application;
pub mod association;
pub mod discovery;
pub mod disk_usage;
pub mod orphans;
pub mod scanner;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub path: PathBuf,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report<T> {
    pub items: Vec<T>,
    pub issues: Vec<Issue>,
}

pub mod containers;

pub mod launch_agents;
mod plist_io;
