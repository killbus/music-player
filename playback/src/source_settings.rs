use crate::source_resolver::SourceResolver;
use music_player_settings::EmbyRuntimeSettings;
use music_player_storage::Database;

impl SourceResolver {
    /// All daemon/API entry points use the same redirect policy and device ID.
    pub fn from_settings(db: Database) -> Self {
        let settings = EmbyRuntimeSettings::read();
        Self::new(db, settings.device_id, settings.follow_redirects)
    }
}
