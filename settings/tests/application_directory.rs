use music_player_settings::{get_application_directory, read_settings, save_audio_settings, Settings};

#[test]
fn explicit_directory_isolates_settings_and_audio_updates() {
    // An integration test runs in its own process, separate from legacy tests
    // which mutate these environment variables and use the default directory.
    let directory = std::env::temp_dir().join(format!("music-player-settings-{}", uuid::Uuid::new_v4()));
    std::env::set_var("MUSIC_PLAYER_APPLICATION_DIRECTORY", &directory);
    let config = read_settings().unwrap();
    let settings_path = directory.join("settings.toml");
    assert!(settings_path.is_file());
    assert!(config.get_string("database_url").unwrap().contains(directory.to_str().unwrap()));
    assert_eq!(std::path::PathBuf::from(get_application_directory()), directory);

    let mut settings: Settings = config.try_deserialize().unwrap();
    settings.audio.bass = 7;
    save_audio_settings(&settings.audio).unwrap();
    let saved = std::fs::read_to_string(settings_path).unwrap();
    assert!(saved.contains("[audio]"));
    let reloaded: Settings = read_settings().unwrap().try_deserialize().unwrap();
    assert_eq!(reloaded.device_id, settings.device_id);
    assert_eq!(reloaded.audio.bass, 7);
    std::fs::remove_dir_all(directory).unwrap();
}
