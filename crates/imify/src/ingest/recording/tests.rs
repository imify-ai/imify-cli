use serde_json::json;

use super::*;

fn recording_json() -> serde_json::Value {
    json!({
        "name": "recordings/3f2c9a",
        "displayName": "pick-place",
        "robot": "so100-0042",
        "operator": "user-1234",
        "startTime": "2026-11-18T09:00:00Z",
        "site": "lab-munich",
        "task": "Pick up the red cube",
        "labels": {"shift": "night"},
        "state": "PROCESSING",
        "format": "v3",
        "robotType": "so100",
        "fps": 30,
        "episodeCount": 12,
        "dataset": "datasets/d-41",
        "createTime": "2026-11-18T09:30:00Z",
        "updateTime": "2026-11-18T09:31:00Z",
        "etag": "W/\"2\""
    })
}

fn parse(value: serde_json::Value) -> Recording {
    Recording::from(serde_json::from_value::<WireRecording>(value).unwrap())
}

#[test]
fn recording_decodes_every_field() {
    assert_eq!(
        parse(recording_json()),
        Recording {
            id: RecordingId::new("3f2c9a"),
            display_name: "pick-place".into(),
            task: "Pick up the red cube".into(),
            robot: "so100-0042".into(),
            operator: "user-1234".into(),
            site: "lab-munich".into(),
            start_time: Some("2026-11-18T09:00:00Z".into()),
            labels: BTreeMap::from([("shift".into(), "night".into())]),
            state: RecordingState::Processing,
            format: Some("v3".into()),
            robot_type: Some("so100".into()),
            fps: Some(30),
            episode_count: 12,
            dataset_id: Some("d-41".into()),
            created_at: "2026-11-18T09:30:00Z".into(),
        }
    );
}

#[test]
fn recording_with_unknown_fields_deserializes() {
    let mut value = recording_json();
    value["checksumAlgorithm"] = json!("SHA256");

    assert_eq!(parse(value).id, RecordingId::new("3f2c9a"));
}

#[test]
fn uploading_recording_has_no_format_fps_or_dataset() {
    let mut value = recording_json();
    value["state"] = json!("UPLOADING");
    value["format"] = json!("");
    value["robotType"] = json!("");
    value["fps"] = json!(0);
    value["dataset"] = json!(null);

    let recording = parse(value);

    assert_eq!(
        (
            recording.format,
            recording.robot_type,
            recording.fps,
            recording.dataset_id
        ),
        (None, None, None, None)
    );
}

#[test]
fn known_states_map_to_their_variants() {
    let states: Vec<RecordingState> = ["UPLOADING", "PROCESSING", "READY"]
        .into_iter()
        .map(|state| {
            let mut value = recording_json();
            value["state"] = json!(state);
            parse(value).state
        })
        .collect();

    assert_eq!(
        states,
        [
            RecordingState::Uploading,
            RecordingState::Processing,
            RecordingState::Ready
        ]
    );
}

#[test]
fn unknown_state_deserializes_as_other_in_lowercase() {
    let mut value = recording_json();
    value["state"] = json!("ARCHIVED");

    let state = parse(value).state;

    assert_eq!(
        (state.clone(), state.as_str().to_owned()),
        (RecordingState::Other("archived".into()), "archived".into())
    );
}

#[test]
fn create_body_holds_the_request_id_name_and_task_only_by_default() {
    let recording = NewRecording::new("pick-place").task("Pick up the red cube");

    assert_eq!(
        recording.to_json(&RequestId::new("0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13")),
        json!({
            "requestId": "0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13",
            "displayName": "pick-place",
            "task": "Pick up the red cube"
        })
    );
}

#[test]
fn create_body_holds_every_option_that_is_set() {
    let recording = NewRecording::new("pick-place")
        .robot("so100-0042")
        .operator("op-17")
        .site("lab-munich")
        .start_time("2026-11-18T09:00:00+01:00")
        .label("shift", "night");

    let body = recording.to_json(&RequestId::new("r"));

    assert_eq!(
        (
            &body["robot"],
            &body["operator"],
            &body["site"],
            &body["startTime"],
            &body["labels"]
        ),
        (
            &json!("so100-0042"),
            &json!("op-17"),
            &json!("lab-munich"),
            &json!("2026-11-18T09:00:00+01:00"),
            &json!({"shift": "night"})
        )
    );
}

#[test]
fn random_request_ids_are_distinct_version_4_uuids() {
    let first = RequestId::random().unwrap();
    let second = RequestId::random().unwrap();

    let is_v4 = |id: &RequestId| {
        let text = id.as_str();
        let groups: Vec<usize> = text.split('-').map(str::len).collect();
        groups == [8, 4, 4, 4, 12]
            && text.as_bytes()[14] == b'4'
            && matches!(text.as_bytes()[19], b'8' | b'9' | b'a' | b'b')
    };
    assert!(is_v4(&first) && is_v4(&second) && first != second);
}

fn fields_of(recording: &NewRecording) -> Vec<String> {
    match recording.check() {
        Ok(()) => Vec::new(),
        Err(Error::Validation { fields, .. }) => fields
            .iter()
            .map(|field| field.location.join("."))
            .collect(),
        Err(other) => panic!("unexpected {other:?}"),
    }
}

#[test]
fn check_accepts_values_at_their_limits() {
    let recording = NewRecording::new("n".repeat(128))
        .task("t".repeat(512))
        .robot("R".repeat(128))
        .operator("op.17_a-b")
        .site("lab-munich")
        .start_time("2026-11-18T09:00:00.123Z")
        .label(format!("a{}", "b".repeat(62)), "v".repeat(256));

    assert_eq!(fields_of(&recording), Vec::<String>::new());
}

#[test]
fn check_names_every_value_outside_its_limit() {
    let recording = NewRecording::new("n".repeat(129))
        .task("t".repeat(513))
        .robot("so100 #42")
        .operator("")
        .site("s".repeat(129))
        .start_time("2026-11-18 09:00")
        .label("Shift", "night")
        .label("x", "v".repeat(257));

    assert_eq!(
        fields_of(&recording),
        [
            "displayName",
            "task",
            "robot",
            "operator",
            "site",
            "startTime",
            "labels.Shift",
            "labels.x"
        ]
    );
}

#[test]
fn check_counts_characters_not_bytes() {
    let recording = NewRecording::new("ü".repeat(128));

    assert_eq!(fields_of(&recording), Vec::<String>::new());
}

#[test]
fn check_refuses_more_than_64_labels() {
    let recording = (0..65).fold(NewRecording::new("pick-place"), |recording, index| {
        recording.label(format!("l{index}"), "v")
    });

    assert_eq!(fields_of(&recording), ["labels"]);
}

#[test]
fn rfc3339_accepts_utc_offsets_and_fractions() {
    for valid in [
        "2026-11-18T09:00:00Z",
        "2026-11-18t09:00:00z",
        "2026-11-18T09:00:00.5+01:00",
        "2026-02-30T23:59:60-12:30",
    ] {
        assert!(is_rfc3339(valid), "{valid}");
    }
}

#[test]
fn rfc3339_refuses_other_formats() {
    for invalid in [
        "2026-11-18",
        "2026-11-18T09:00:00",
        "2026-11-18 09:00:00Z",
        "2026-13-18T09:00:00Z",
        "2026-11-00T09:00:00Z",
        "2026-11-18T24:00:00Z",
        "2026-11-18T09:00:00.Z",
        "2026-11-18T09:00:00+0100",
        "2026-11-18T09:00:00+24:00",
        "2026-11-18T09:00:00+01:00x",
        "２０２６-11-18T09:00:00Z",
    ] {
        assert!(!is_rfc3339(invalid), "{invalid}");
    }
}

#[test]
fn custom_method_url_appends_the_verb_with_a_colon() {
    let client = Client::builder("https://api.example.com").build().unwrap();

    let url = method_url(&client, &RecordingId::new("3f2c9a"), "complete").unwrap();

    assert_eq!(
        url.as_str(),
        "https://api.example.com/api/v1/recordings/3f2c9a:complete"
    );
}

#[test]
fn recording_url_encodes_the_id_as_one_segment() {
    let client = Client::builder("https://api.example.com").build().unwrap();

    let url = recording_url(
        &client,
        &RecordingId::new("a/b c"),
        Some("files:batchCreate"),
    )
    .unwrap();

    assert_eq!(
        url.as_str(),
        "https://api.example.com/api/v1/recordings/a%2Fb%20c/files:batchCreate"
    );
}
