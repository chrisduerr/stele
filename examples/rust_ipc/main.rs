use std::thread;
use std::time::Duration;

use chrono::Local;
use stele_ipc::{
    self, Alignment, Color, Config, IpcMessage, LayerContent, Margin, Module, ModuleLayer,
};

fn main() {
    // Initialize the time module.
    stele_ipc::send_message(&IpcMessage::Module(time_module()));

    // Show the bar.
    stele_ipc::send_message(&IpcMessage::Config(config()));

    loop {
        thread::sleep(Duration::from_secs(1));
        stele_ipc::send_message(&IpcMessage::Module(time_module()));
    }
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
