use mf_chat::{ChatDatabase, ChatHost, HostConfig, WorkflowProfile};
use mfn_llm::test_support::{Reply, Server, messages};
use serde_json::json;

fn profile(server: &Server) -> WorkflowProfile {
    let definition = json!({
        "version":"2026-11-13", "dependencies":{},
        "nodes":[
            {"id":"params","kind":"builtin.parameters","config":{"parameters":{
                "messages":{"type":{"list":"object"}},"api_key":{"type":"string","default":""}
            }}},
            {"id":"model","kind":"builtin.llm","config":{
                "model":"fixture-model","base_url":server.base_url,"api_key":{"$ref":"params.api_key"}
            },"inputs":{"messages":{"$ref":"params.messages"}}},
            {"id":"compact","kind":"builtin.code","config":{"language":"cel","inputs":{"messages":{"list":"object"}},
                "code":{"value":"messages[messages.size() - 1].content == 'compact'"}
            },"inputs":{"messages":{"$ref":"params.messages"}}},
            {"id":"route","kind":"builtin.if_else","config":{"branches":[{"id":"compact","condition":{
                "source":{"output":"compact.value","path":""},"operator":"eq","value":true
            }}]}},
            {"id":"snapshot","kind":"builtin.constant","config":{"value":[{"role":"user","content":"Only the summary"}]}},
            {"id":"normal","kind":"builtin.object","config":{"fields":{
                "status":"string","messages":{"list":"object"},"usages":{"list":"object"}
            }},"inputs":{
                "status":{"$ref":"model.status"},
                "messages":{"$ref":"model.messages"},
                "usages":{"$ref":"model.usages"}
            }},
            {"id":"replacement","kind":"builtin.object","config":{"fields":{
                "status":"string","context_messages":{"list":"object"},"display_messages":{"list":"object"},"usages":{"list":"object"}
            }},"inputs":{
                "status":{"$ref":"model.status"},
                "context_messages":{"$ref":"snapshot.value"},
                "display_messages":{"$ref":"model.messages"},
                "usages":{"$ref":"model.usages"}
            }}
        ],
        "edges":[
            {"from_node":"compact","to_node":"route"},
            {"from_node":"route","from_output":"else","to_node":"normal"},
            {"from_node":"route","from_output":"compact","to_node":"replacement"}
        ],
        "outputs":[
            {"name":"append","node":"normal","port":"value","optional":true},
            {"name":"replace_context","node":"replacement","port":"value","optional":true},
            {"name":"answer","node":"model","port":"delta","optional":true}
        ]
    });
    WorkflowProfile::new(
        "default",
        serde_json::from_value(definition).unwrap(),
        Default::default(),
    )
    .unwrap()
}

#[test]
fn independent_branch_outputs_reach_real_provider_requests_after_reopening() {
    let server = Server::start(vec![
        Reply::stream_text("First answer"),
        Reply::stream_text("Compacted answer"),
        Reply::stream_text("Third answer"),
    ]);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("chat.sqlite");
    let profile = profile(&server);
    let host = ChatHost::open(&path, HostConfig::new(profile.clone())).unwrap();
    let id = host
        .database()
        .create_session("default", Some("Original instructions".into()), None)
        .unwrap()
        .id;
    host.submit(&id, "normal").unwrap().join().unwrap();
    host.submit(&id, "compact").unwrap().join().unwrap();
    assert_eq!(
        host.database().model_context(&id).unwrap().messages.len(),
        1
    );
    let original = host
        .database()
        .messages(&id, 50, None)
        .unwrap()
        .messages
        .into_iter()
        .map(|record| record.message)
        .collect::<Vec<_>>();
    assert_eq!(original.len(), 4);
    assert_eq!(original[3].content(), "Compacted answer");
    host.close().unwrap();
    let host = ChatHost::open(&path, HostConfig::new(profile)).unwrap();
    host.submit(&id, "continue").unwrap().join().unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        messages(&requests[0]),
        vec![
            json!({"role":"system","content":"Original instructions"}),
            json!({"role":"user","content":"normal"})
        ]
    );
    assert_eq!(
        messages(&requests[1]),
        vec![
            json!({"role":"system","content":"Original instructions"}),
            json!({"role":"user","content":"normal"}),
            json!({"role":"assistant","content":"First answer"}),
            json!({"role":"user","content":"compact"})
        ]
    );
    assert_eq!(
        messages(&requests[2]),
        vec![
            json!({"role":"user","content":"Only the summary"}),
            json!({"role":"user","content":"continue"})
        ]
    );
    let page = host.database().messages(&id, 50, None).unwrap();
    assert_eq!(
        page.messages[..4]
            .iter()
            .map(|record| &record.message)
            .collect::<Vec<_>>(),
        original.iter().collect::<Vec<_>>()
    );
    assert_eq!(page.messages.len(), 6);
    assert_eq!(
        host.database()
            .usage(Default::default())
            .unwrap()
            .overall
            .attempted_calls,
        3
    );
    host.close().unwrap();
}

#[test]
fn zero_or_multiple_produced_results_cannot_become_recovery_candidates() {
    for produce in [false, true] {
        let payload = json!({"status":"succeeded","messages":[{"role":"assistant","content":"Never saved"}],"usages":[]});
        let mut definition = json!({
            "version":"2026-11-13", "dependencies":{},
            "nodes":[
                {"id":"params","kind":"builtin.parameters","config":{"parameters":{"messages":{"type":{"list":"object"}}}}},
                {"id":"enabled","kind":"builtin.constant","config":{"value":produce}},
                {"id":"route","kind":"builtin.if_else","config":{"branches":[{"id":"go","condition":{"source":{"output":"enabled.value","path":""},"operator":"eq","value":true}}]}},
                {"id":"normal","kind":"builtin.constant","config":{"value":payload}},
                {"id":"replacement","kind":"builtin.constant","config":{"value":{
                    "status":"succeeded","context_messages":[],"display_messages":[{"role":"assistant","content":"Never saved"}],"usages":[]
                }}}
            ],
            "edges":[
                {"from_node":"enabled","to_node":"route"},
                {"from_node":"route","from_output":"go","to_node":"normal"},
                {"from_node":"route","from_output":"go","to_node":"replacement"}
            ],
            "outputs":[{"name":"append","node":"normal","port":"value","optional":true},{"name":"replace_context","node":"replacement","port":"value","optional":true}]
        });
        let profile = WorkflowProfile::new(
            "default",
            serde_json::from_value::<mf_runtime::WorkflowDefinition>(definition.take()).unwrap(),
            Default::default(),
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let host =
            ChatHost::open(root.path().join("chat.sqlite"), HostConfig::new(profile)).unwrap();
        let id = host
            .database()
            .create_session("default", None, None)
            .unwrap()
            .id;
        let error = host.submit(&id, "Question").unwrap().join().unwrap_err();
        assert!(!matches!(error, mf_chat::ChatError::Uncommitted { .. }));
        assert_eq!(
            host.database()
                .messages(&id, 50, None)
                .unwrap()
                .messages
                .len(),
            1
        );
        assert!(
            host.database()
                .model_context(&id)
                .unwrap()
                .messages
                .is_empty()
        );
        host.close().unwrap();
        let database = ChatDatabase::open(root.path().join("chat.sqlite")).unwrap();
        assert_eq!(database.session(&id).unwrap().context_revision, 0);
    }
}
