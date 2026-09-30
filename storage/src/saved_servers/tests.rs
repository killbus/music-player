use super::*;
use migration::{Migrator, MigratorTrait};
use sea_orm::Database;

const NOW: &str = "2026-09-30T10:00:00Z";
const LATER: &str = "2026-09-30T11:00:00Z";

async fn db() -> DatabaseConnection {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    Migrator::up(&db, None).await.unwrap();
    db
}

fn account(user: &str) -> NewServer {
    NewServer::new("jellyfin", "Home", "http://emby:8096/")
        .with_credentials(Some(user.into()), Some(format!("password-{user}")))
}

#[tokio::test]
async fn migration_retains_old_id_and_replaces_url_uniqueness() {
    let db = Database::connect("sqlite::memory:").await.unwrap();
    // Apply the actual old schema, then insert an old deterministic ID.
    let old_count = Migrator::migrations()
        .iter()
        .position(|migration| migration.name() == "m20260930_000001_saved_accounts")
        .unwrap();
    Migrator::up(&db, Some(old_count as u32)).await.unwrap();
    let old_id = format!("{:x}", md5::compute("jellyfin\0http://emby:8096"));
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Sqlite,
        "INSERT INTO saved_server(id,kind,name,url,username,password,created_at,updated_at)
        VALUES (?, 'jellyfin','Old','http://emby:8096','family','old-secret',?,?)",
        [old_id.clone().into(), NOW.into(), NOW.into()],
    ))
    .await
    .unwrap();
    db.execute_unprepared(
        "INSERT INTO saved_server(id,kind,name,url,username) VALUES
        ('empty-user','music-player','Peer','http://peer','　'),
        ('spaced-user','jellyfin','Other','http://other',' family ')",
    )
    .await
    .unwrap();
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        get(&db, "empty-user").await.unwrap().unwrap().username,
        None
    );
    assert_eq!(
        get(&db, "spaced-user")
            .await
            .unwrap()
            .unwrap()
            .username
            .as_deref(),
        Some(" family ")
    );
    delete(&db, "empty-user").await.unwrap();
    delete(&db, "spaced-user").await.unwrap();
    let existing = get(&db, &old_id).await.unwrap().unwrap();
    let saved = upsert(
        &db,
        &account("family").with_password_update(PasswordUpdate::Keep),
        LATER,
    )
    .await
    .unwrap();
    assert_eq!(saved.id, old_id);
    assert_eq!(saved.created_at, existing.created_at);
    assert_eq!(saved.password, existing.password);
    let other = upsert(&db, &account("guest"), LATER).await.unwrap();
    assert_ne!(other.id, old_id);
    assert!(Uuid::parse_str(&other.id).is_ok());
    // A downgrade cannot drop either account to restore the former index.
    assert!(Migrator::down(&db, Some(1)).await.is_err());
    assert_eq!(list(&db).await.unwrap().len(), 2);
    assert!(delete(&db, &old_id).await.unwrap());
    assert!(get(&db, &old_id).await.unwrap().is_none());
    assert_eq!(get(&db, &other.id).await.unwrap(), Some(other));
}

#[tokio::test]
async fn same_address_accounts_never_inherit_each_others_password() {
    let db = db().await;
    let a = upsert(&db, &account("a"), NOW).await.unwrap();
    let b = upsert(
        &db,
        &account("b").with_password_update(PasswordUpdate::Keep),
        NOW,
    )
    .await
    .unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(b.password, None);
    let again = upsert(
        &db,
        &account("a").with_password_update(PasswordUpdate::Keep),
        LATER,
    )
    .await
    .unwrap();
    assert_eq!(again.id, a.id);
    assert_eq!(again.password, a.password);
    assert_eq!(again.created_at, a.created_at);
    assert_eq!(again.updated_at.as_deref(), Some(LATER));
    let spaced = upsert(&db, &account(" a "), NOW).await.unwrap();
    assert_eq!(spaced.username.as_deref(), Some(" a "));
    assert_ne!(spaced.id, a.id);
    assert_eq!(list(&db).await.unwrap().len(), 3);
}

#[tokio::test]
async fn explicit_edits_keep_identity_and_reject_retargeting_or_collision() {
    let db = db().await;
    let a = upsert(&db, &account("family"), NOW).await.unwrap();
    let mut edit = account("family").with_id(Some(a.id.clone()));
    edit.name = "Renamed".into();
    edit.url = " http://new-address:8096/ ".into();
    edit.password_update = PasswordUpdate::Set(String::new());
    let moved = upsert(&db, &edit, LATER).await.unwrap();
    assert_eq!(moved.id, a.id);
    assert_eq!(moved.created_at, a.created_at);
    assert_eq!(moved.url, "http://new-address:8096");
    assert_eq!(moved.name, "Renamed");
    assert_eq!(moved.password.as_deref(), Some(""));
    let original_address = upsert(&db, &account("family"), NOW).await.unwrap();
    edit.url = original_address.url.clone();
    assert!(upsert(&db, &edit, LATER).await.is_err());
    assert_eq!(get(&db, &moved.id).await.unwrap(), Some(moved.clone()));
    assert_eq!(
        get(&db, &original_address.id).await.unwrap(),
        Some(original_address)
    );
    edit.url = moved.url.clone();
    edit.username = Some("other".into());
    assert!(upsert(&db, &edit, LATER).await.is_err());
    edit.username = moved.username.clone();
    edit.kind = "subsonic".into();
    assert!(upsert(&db, &edit, LATER).await.is_err());
    assert_eq!(get(&db, &moved.id).await.unwrap(), Some(moved));
    edit.id = Some("missing".into());
    assert!(upsert(&db, &edit, LATER).await.is_err());
}

#[tokio::test]
async fn deletion_never_reuses_an_old_handle() {
    let db = db().await;
    let a = upsert(&db, &account("family"), NOW).await.unwrap();
    assert!(delete(&db, &a.id).await.unwrap());
    assert!(!delete(&db, &a.id).await.unwrap());
    let b = upsert(&db, &account("family"), NOW).await.unwrap();
    assert_ne!(a.id, b.id);
    assert!(get(&db, &a.id).await.unwrap().is_none());
}

#[tokio::test]
async fn password_keep_set_empty_and_clear_survive_storage() {
    let db = db().await;
    let a = upsert(&db, &account("family"), NOW).await.unwrap();
    let legacy_empty =
        account("family").with_credentials(Some("family".into()), Some(String::new()));
    assert_eq!(
        upsert(&db, &legacy_empty, LATER).await.unwrap().password,
        a.password
    );
    for id in [None, Some(a.id.clone())] {
        let input = account("family").with_id(id);
        for (update, expected) in [
            (PasswordUpdate::Set(String::new()), Some("")),
            (PasswordUpdate::Keep, Some("")),
            (PasswordUpdate::Set("   ".into()), Some("   ")),
            (PasswordUpdate::Clear, None),
            (PasswordUpdate::Keep, None),
        ] {
            let saved = upsert(&db, &input.clone().with_password_update(update), LATER)
                .await
                .unwrap();
            assert_eq!(saved.password.as_deref(), expected);
        }
    }
}

#[test]
fn shared_api_password_compatibility_and_conflicts() {
    for legacy in [None, Some(String::new())] {
        assert_eq!(
            PasswordUpdate::from_fields(legacy.clone(), None, false).unwrap(),
            PasswordUpdate::Keep
        );
        assert_eq!(
            PasswordUpdate::from_fields(legacy.clone(), Some(String::new()), false).unwrap(),
            PasswordUpdate::Set(String::new())
        );
        assert_eq!(
            PasswordUpdate::from_fields(legacy, None, true).unwrap(),
            PasswordUpdate::Clear
        );
    }
    assert_eq!(
        PasswordUpdate::from_fields(Some("p".into()), None, false).unwrap(),
        PasswordUpdate::Set("p".into())
    );
    assert!(PasswordUpdate::from_fields(None, Some(String::new()), true).is_err());
    assert!(PasswordUpdate::from_fields(Some("p".into()), Some(String::new()), false).is_err());
    assert!(PasswordUpdate::from_fields(Some("p".into()), None, true).is_err());
    assert!(!format!("{:?}", account("family")).contains("password-family"));
}

#[tokio::test]
async fn absent_username_has_database_enforced_uniqueness() {
    let db = db().await;
    let no_user = NewServer::new("music-player", "Peer", "http://peer");
    let a = upsert(&db, &no_user, NOW).await.unwrap();
    for username in ["", " \t\n", "　"] {
        let b = upsert(
            &db,
            &no_user
                .clone()
                .with_credentials(Some(username.into()), None),
            NOW,
        )
        .await
        .unwrap();
        assert_eq!(a.id, b.id);
    }
    // Bypassing the helper must still fail for SQL NULL versus empty username.
    assert!(db
        .execute_unprepared(
            "INSERT INTO saved_server(id,kind,name,url,username)
        VALUES ('bypass','music-player','Peer','http://peer','')"
        )
        .await
        .is_err());
    assert_eq!(list(&db).await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_saves_share_one_id_without_losing_password() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("accounts.db");
    let db = Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    crate::enable_wal(&db).await;
    let mut tasks = Vec::new();
    for index in 0..12 {
        let db = db.clone();
        tasks.push(tokio::spawn(async move {
            let update = if index == 0 {
                PasswordUpdate::Set("chosen".into())
            } else {
                PasswordUpdate::Keep
            };
            upsert(&db, &account("family").with_password_update(update), NOW)
                .await
                .unwrap()
                .id
        }));
    }
    let mut ids = std::collections::HashSet::new();
    for task in tasks {
        ids.insert(task.await.unwrap());
    }
    assert_eq!(ids.len(), 1);
    let rows = list(&db).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].password.as_deref(), Some("chosen"));
}

#[tokio::test]
async fn legacy_import_is_atomic_non_overwriting_and_never_resurrects_accounts() {
    let db = db().await;
    let current = upsert(&db, &account("family"), NOW).await.unwrap();
    let legacy = vec![
        account("family").with_password_update(PasswordUpdate::Set("stale".into())),
        account("guest"),
        account("guest").with_password_update(PasswordUpdate::Set("lower-priority".into())),
    ];
    assert_eq!(
        import_legacy_accounts(&db, &legacy, LATER).await.unwrap(),
        1
    );
    assert!(legacy_imported(&db).await.unwrap());
    assert_eq!(get(&db, &current.id).await.unwrap(), Some(current.clone()));
    let guest = list(&db)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.username.as_deref() == Some("guest"))
        .unwrap();
    assert_eq!(guest.password.as_deref(), Some("password-guest"));
    delete(&db, &guest.id).await.unwrap();
    let mut edited = account("family").with_id(Some(current.id));
    edited.url = "http://moved".into();
    let edited = upsert(&db, &edited, LATER).await.unwrap();
    assert_eq!(
        import_legacy_accounts(&db, &legacy, LATER).await.unwrap(),
        0
    );
    assert_eq!(list(&db).await.unwrap(), vec![edited]);
}

#[tokio::test]
async fn failed_import_rolls_back_rows_and_marker_then_retries() {
    let db = db().await;
    db.execute_unprepared(
        "CREATE TRIGGER reject_guest BEFORE INSERT ON saved_server
        WHEN NEW.username = 'guest' BEGIN SELECT RAISE(ABORT,'fixture failure'); END",
    )
    .await
    .unwrap();
    let accounts = [account("family"), account("guest")];
    assert!(import_legacy_accounts(&db, &accounts, NOW).await.is_err());
    assert!(!legacy_imported(&db).await.unwrap());
    assert!(list(&db).await.unwrap().is_empty());
    db.execute_unprepared("DROP TRIGGER reject_guest")
        .await
        .unwrap();
    assert_eq!(
        import_legacy_accounts(&db, &accounts, NOW).await.unwrap(),
        2
    );
    assert!(legacy_imported(&db).await.unwrap());
}

#[test]
fn legacy_json_failure_preserves_input_and_empty_password_remains_compatible() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("desktop_servers.json");
    assert!(legacy_json_servers(Some(&path)).unwrap().is_empty());
    std::fs::write(&path, "{broken").unwrap();
    assert!(legacy_json_servers(Some(&path)).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    std::fs::write(&path, r#"[{"kind":"jellyfin","name":"Home","url":"http://emby","username":"family","password":""}]"#).unwrap();
    let accounts = legacy_json_servers(Some(&path)).unwrap();
    assert_eq!(accounts[0].username.as_deref(), Some("family"));
    assert_eq!(accounts[0].password_update, PasswordUpdate::Keep);
}
