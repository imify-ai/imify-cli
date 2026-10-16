use serde_json::json;

use super::*;

fn session_json() -> serde_json::Value {
    json!({
        "id": "3f2c9a",
        "name": "pick-place",
        "dataset_format": "lerobot_v3",
        "robot_type": "so100",
        "fps": 30.0,
        "total_episodes": 12,
        "status": "uploading",
        "task_description": "Pick up the red cube",
        "created_at": "2026-10-16T09:30:00Z"
    })
}

fn parse(value: serde_json::Value) -> Session {
    Session::from(serde_json::from_value::<WireSession>(value).unwrap())
}

#[test]
fn session_with_unknown_fields_deserializes() {
    let mut value = session_json();
    value["owner"] = json!({"id": "usr_9"});

    assert_eq!(parse(value).id, SessionId::new("3f2c9a"));
}

#[test]
fn null_format_fields_deserialize_as_none() {
    let mut value = session_json();
    for field in ["dataset_format", "robot_type", "fps", "total_episodes"] {
        value[field] = serde_json::Value::Null;
    }

    let session = parse(value);

    assert_eq!(
        (
            session.dataset_format,
            session.robot_type,
            session.fps,
            session.total_episodes
        ),
        (None, None, None, None)
    );
}

#[test]
fn unknown_status_deserializes_as_other() {
    let mut value = session_json();
    value["status"] = json!("cataloguing");

    assert_eq!(
        parse(value).status,
        SessionStatus::Other("cataloguing".into())
    );
}

#[test]
fn ready_status_deserializes_as_ready() {
    let mut value = session_json();
    value["status"] = json!("ready");

    assert_eq!(parse(value).status, SessionStatus::Ready);
}

#[test]
fn new_session_serializes_name_and_task_description() {
    let session = NewSession::new("pick-place").task_description("Pick up the red cube");

    assert_eq!(
        session.to_json(),
        json!({"name": "pick-place", "task_description": "Pick up the red cube"})
    );
}

#[test]
fn session_url_percent_encodes_each_segment() {
    let client = Client::builder("https://api.example.com/base")
        .build()
        .unwrap();

    let url = session_url(
        &client,
        &SessionId::new("3f2c9a"),
        ["files", "data", "chunk 000", "file.parquet"],
    )
    .unwrap();

    assert_eq!(
        url.as_str(),
        "https://api.example.com/base/api/v1/ingest/sessions/3f2c9a/files/data/chunk%20000/file.parquet"
    );
}
