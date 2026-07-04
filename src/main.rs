mod app;
mod config;
mod history;
mod model;
mod preview;
mod shell;
mod sources;
mod state;
#[cfg(test)]
mod test_support;
mod ui;

fn main() {
    app::run();
}
