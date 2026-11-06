use super::setup_schema;
use async_graphql::Request;
use music_player_types::types::Device as DiscoveryDevice;
use std::sync::Arc;

fn cast_device(id: &str, service: &str) -> DiscoveryDevice {
    DiscoveryDevice {
        id: id.to_string(),
        name: format!("device-{id}"),
        host: "127.0.0.1".to_string(),
        ip: "127.0.0.1".to_string(),
        port: 5053,
        service: service.to_string(),
        app: "music-player".to_string(),
        is_cast_device: true,
        is_source_device: false,
        ..Default::default()
    }
}

#[tokio::test]
async fn list_cast_devices_hides_self_and_keeps_peer() {
    let (schema, _, _, _, _) = setup_schema().await;
    let daemon_id = music_player_settings::read_settings()
        .unwrap()
        .get_string("device_id")
        .unwrap();
    let devices = Arc::new(std::sync::Mutex::new(vec![
        cast_device(&daemon_id, "grpc"),
        cast_device(&daemon_id, "http"),
        cast_device("peer-id", "grpc"),
        cast_device("peer-id", "http"),
    ]));
    let response = schema
        .execute(Request::new("query { listCastDevices { id service } }"))
        .data(devices)
        .await;
    assert!(response.errors.is_empty(), "{:?}", response.errors);
    let rows = response.data.into_json().unwrap()["listCastDevices"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "peer-id");
}

#[tokio::test]
async fn connecting_to_self_is_rejected_before_receiver_state_changes() {
    let (schema, _, _, _, _) = setup_schema().await;
    let daemon_id = music_player_settings::read_settings()
        .unwrap()
        .get_string("device_id")
        .unwrap();
    let devices = Arc::new(std::sync::Mutex::new(vec![cast_device(&daemon_id, "grpc")]));
    let response = schema
        .execute(Request::new(
            "mutation($id: ID!) { connectToCastDevice(id: $id) { id } }",
        ))
        .variables(async_graphql::Variables::from_value(
            async_graphql::value!({ "id": daemon_id }),
        ))
        .data(devices)
        .await;
    assert_eq!(response.errors.len(), 1);
    assert!(response.errors[0]
        .message
        .contains("current daemon is the local output"));
}
