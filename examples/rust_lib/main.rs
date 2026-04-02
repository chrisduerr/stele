use std::time::Duration;

use chrono::Local;
use stele::calloop::timer::{TimeoutAction, Timer};
use stele::{Alignment, Color, Config, LayerContent, Margin, Module, ModuleLayer, Stele};

fn main() {
    let mut stele = Stele::new().unwrap();

    // Initialize the time module.
    stele.state().update_module(time_module());

    // Show the bar.
    stele.state().update_config(config());

    // Update bar every second.
    let timer = Timer::from_duration(Duration::from_secs(1));
    stele
        .event_loop()
        .insert_source(timer, |_, _, state| {
            state.update_module(time_module());
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .unwrap();

    stele.run().unwrap();
}

/// Global configuration
fn config() -> Config {
    // Use #181818 as background color.
    let background = LayerContent::Color(Color::new(24, 24, 24));

    let mut config = Config::new();
    config.backgrounds = vec![background];

    config
}

/// Time module configuration.
fn time_module() -> Module {
    // Use #282828 as background color.
    let background = ModuleLayer::new(LayerContent::Color(Color::new(40, 40, 40)));

    // Add time text with a small background color margin at the left/right.
    let time = Local::now().format("%H:%M:%S").to_string();
    let mut text = ModuleLayer::new(LayerContent::Text(time.into()));
    text.margin = Margin { left: 25, right: 25 };

    let layers = vec![background, text];
    Module::new("time_module", Alignment::Center, layers)
}
