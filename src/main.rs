use std::fs::File;
use media_player::{Config, MediaLibrary};

fn main() {
    // If we can't find the user's base directories, we can't load the config.
    let base_dirs = directories::BaseDirs::new().expect("Could not find user's BaseDirs");
    let mut config_dir = base_dirs.config_dir().to_path_buf();
    config_dir.push("Media Player");
    config_dir.push("config.json");

    let config_file = File::open(config_dir);
    let mut config = if let Ok(file) = config_file {
        // If we can't parse the config file, we don't want to exit instead of loading a default config file
        serde_json::from_reader(file).expect("Could not parse config file")
    } else {
        // the config file does not exist or is unreadable so use the default config.
        Config::default()
    };

    if config.data_directory.is_empty() {
        // I'm not sure when this would ever fail in practice even though it theoretically could.'
        let mut data_dir = base_dirs.data_dir().to_path_buf();
        data_dir.push("Nested Playlist Media Player");
        config.data_directory = data_dir.to_str().expect("Could not parse data directory as a string").to_owned();
    }

    let library = MediaLibrary::load(config);
}