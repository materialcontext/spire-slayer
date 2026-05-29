mod data;
mod domain;
mod input;
mod metrics;
mod sim;
mod telemetry;
mod tui;

fn main() -> anyhow::Result<()> {
    tui::run_app()
}
