mod app;
mod config;
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
