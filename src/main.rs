use std::fs::File;
use std::sync::{OnceLock, RwLock};
use media_player::{Config, MediaLibrary};

pub static MEDIA_LIBRARY: OnceLock<RwLock<MediaLibrary>> = OnceLock::new();

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
        let mut data_dir = base_dirs.data_dir().to_path_buf();
        data_dir.push("Nested Playlist Media Player");
        // I'm not sure when this would ever fail in practice even though it theoretically could.'
        config.data_directory = data_dir.to_str().expect("Could not parse data directory as a string").to_owned();
    }

    let library = MediaLibrary::load(config);
    let library = match library {
        Ok(library) => library,
        Err(error) => {
            todo!("Handle errors when loading media library: {}", error);
        }
    };

    // MEDIA_LIBRARY is guaranteed to be uninitialized here, so this will never fail.
    MEDIA_LIBRARY.set(RwLock::new(library)).ok().unwrap();
}