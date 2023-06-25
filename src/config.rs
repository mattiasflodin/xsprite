use std::fs;
use anyhow::anyhow;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Config {
    pub(crate) init_keyboard: Option<String>,
}

pub(crate) fn load_config() -> anyhow::Result<Config> {
    // Load config file from user's local .config dir
    let path = match dirs::config_dir() {
        Some(path) => path,
        None => {
            return Err(anyhow!("failed to find user's config dir"));
        }
    };

    let path = path.join("xsprite").join("config.toml");
    let contents = fs::read_to_string(&path)
        .map_err(|err| {
            anyhow!("failed to read config file {}: {}", path.display(), err)
        })?;
    let mut config: Config = toml::from_str(&contents)
        .map_err(|err| {
            anyhow!("failed to parse config file {}: {}", path.display(), err)
        })?;

    // Resolve ~ and environment variables in init_keyboard
    if let Some(init_keyboard) = &mut config.init_keyboard {
        *init_keyboard = shellexpand::full(init_keyboard)
            .map_err(|err| {
                anyhow!("failed to expand init_keyboard: {}", err)
            })?.into();
    }
    Ok(config)
}
