#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![forbid(unsafe_code)]

use naui::{Result, Settings};

fn main() -> Result<()> {
    start()
}

fn build(ui: &naui::Ui) -> Result<()> {
    prs_app::ui::build(ui)
}

naui::entry!(
    Settings::new("Pull Request Scanner Rust").app_id("com.godlytalias.PullRequestScanner.Rust"),
    build
);
