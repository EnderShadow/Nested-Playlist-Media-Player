mod gui;
mod player;

use media_player::{Config, MediaLibrary};
use std::fs::File;
use std::sync::{OnceLock, RwLock};

pub static MEDIA_LIBRARY: OnceLock<RwLock<MediaLibrary>> = OnceLock::new();

fn main() {
    // If we can't find the user's base directories, we can't load the config.
    let base_dirs = directories::BaseDirs::new().expect("Could not find user's BaseDirs");
    let mut config_path = base_dirs.config_dir().to_path_buf();
    config_path.push("Media Player");
    config_path.push("config.json");

    let config_file = File::open(config_path);
    let mut config = if let Ok(file) = config_file {
        // If we can't parse the config file, we want to use a default config.
        serde_json::from_reader(file).unwrap_or_else(|_| {
            println!("Could not parse config file, using default config");
            Config::default()
        })
    } else {
        // the config file does not exist or is unreadable so use the default config.
        Config::default()
    };

    if config.data_directory.as_os_str().is_empty() {
        let mut data_dir = base_dirs.data_dir().to_path_buf();
        data_dir.push("Nested Playlist Media Player");
        config.data_directory = data_dir;
    }

    let library = MediaLibrary::load(config);
    let mut library = match library {
        Ok(library) => library,
        Err(error) => {
            todo!("Handle errors when loading media library: {:?}", error);
        }
    };

    library.save().unwrap_or_else(|_| {
        println!("Failed to save library after loading");
    });

    // MEDIA_LIBRARY is guaranteed to be uninitialized here, so this will never fail.
    MEDIA_LIBRARY.set(RwLock::new(library)).ok().unwrap();

    let x = gui::run_gui();
    x.unwrap();
}
