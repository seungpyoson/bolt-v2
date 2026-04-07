pub mod clients;
pub mod config;
mod live_config;
pub mod secrets;
pub mod strategies;

pub fn render_live_config_from_path(
    input_path: &std::path::Path,
    output_path: &std::path::Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let input = live_config::LiveLocalConfig::load(input_path)?;
    live_config::render_runtime_config(&input, input_path, output_path)
}
