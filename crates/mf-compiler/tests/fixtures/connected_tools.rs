use serde_json::{Value, json};

pub fn tool_body() -> Value {
    json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"operation","kind":"builtin.code","config":{"language":"cel","inputs":{"query":"string","token":"string"},"code":{"text":"query + token","internal":"'private'"}}},
        {"id":"render","kind":"builtin.code","config":{"language":"cel","inputs":{"value":"string"},"code":{"message":"'reply:' + value"}},"inputs":{"value":{"from":{"node":"operation","port":"text"}}}}
    ],"outputs":[{"name":"text","node":"render","port":"message"}]})
}

pub fn document() -> Value {
    json!({"version":"2026-10-28","dependencies":{},"nodes":[
        {"id":"a_parent","kind":"test.directed_caller","config":{"repeat":2}},
        {"id":"z_config","kind":"builtin.constant","config":{"value":"credential"}},
        {"id":"tool","kind":"workflow.call","config":{"flow":"tool","definition":tool_body()},"inputs":{
            "operation.query":{"from":{"node":"a_parent","port":"query"}},
            "operation.token":{"from":{"node":"z_config","port":"value"}}
        }},
        {"id":"unused","kind":"workflow.call","config":{"flow":"unused","definition":{
            "version":"2026-10-28","dependencies":{},"nodes":[
                {"id":"unreachable","kind":"builtin.code","config":{"language":"cel","inputs":{},"code":{"text":"1 / 0"}}}
            ],"outputs":[{"name":"text","node":"unreachable","port":"text"}]
        }}}
    ],"calls":{"a_parent":{"search":{"target":"tool"},"unused":{"target":"unused"}}},
    "outputs":[{"name":"result","node":"a_parent","port":"value"}]})
}
