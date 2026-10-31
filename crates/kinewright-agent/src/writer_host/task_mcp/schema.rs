//! Wire schemas for internal capabilities. The served MCP tool set stays seven.
use serde_json::{Value, json};
fn object(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({"type":"object","required":required,"additionalProperties":false});
    schema["properties"] = properties;
    schema
}
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/$defs/{name}")})
}
fn array(item: Value, limit: usize) -> Value {
    let mut schema = json!({"type":"array","maxItems":limit});
    schema["items"] = item;
    schema
}
fn definitions() -> Value {
    let id = json!({"type":"string","pattern":"^[0-9a-f]{32}$"});
    let digest = json!({"type":"string","pattern":"^[0-9a-f]{64}$"});
    let refs = array(reference("object_ref"), 256);
    let strings =
        json!({"type":"array","items":{"type":"string"},"uniqueItems":true,"maxItems":4096});
    json!({
        "identity":id,
        "command_id":object(json!({"issuer":reference("identity"),"sequence":{"type":"integer","minimum":1}}),&["issuer","sequence"]),
        "object_ref":object(json!({"sha256":digest,"byte_length":{"type":"integer","minimum":1},"record_kind":{"type":"string"}}),&["sha256","byte_length","record_kind"]),
        "binding":object(json!({"branch":reference("identity"),"head":reference("identity"),"checkpoint":reference("object_ref"),"document_hash":digest}),&["branch","head","checkpoint","document_hash"]),
        "provider":object(json!({"connector":{"type":"string"},"model":{"type":"string"},"options":array(object(json!({"name":{"type":"string"},"value":{"type":"string"}}),&["name","value"]),32)}),&["connector","model","options"]),
        "scope":object(json!({"operations":strings,"objects":strings,"fields":strings,"tracks":strings,"bus_sidechain_closure":strings,"asset_roots":refs,"base":reference("binding"),"consent_refs":refs}),&["operations","objects","fields","tracks","bus_sidechain_closure","asset_roots","base","consent_refs"]),
        "brief":object(json!({"plan":reference("object_ref"),"script":reference("object_ref"),"inputs":reference("object_ref"),"assignment":reference("object_ref"),"role":{"enum":["director","editor","motion","audio","colour","reviewer"]},"provider":reference("provider"),"scope":reference("scope")}),&["plan","script","inputs","assignment","role","provider","scope"]),
        "result":object(json!({"outcome":{"enum":["completed","failed","cancelled","interrupted"]},"base":{"anyOf":[reference("binding"),{"type":"null"}]},"head":{"anyOf":[reference("binding"),{"type":"null"}]},"delta_ref":{"anyOf":[reference("object_ref"),{"type":"null"}]},"artifacts":refs,"evidence":refs,"usage_ref":reference("object_ref"),"explanation":reference("object_ref")}),&["outcome","base","head","delta_ref","artifacts","evidence","usage_ref","explanation"])
    })
}
pub(crate) fn capability(name: &str) -> Value {
    if matches!(name, "task_status" | "wait_task") {
        return object(
            json!({"after_sequence":{"type":"integer","minimum":0}}),
            &[],
        );
    }
    if matches!(name, "undo_task_edit" | "redo_task_edit") {
        return object(
            json!({"expected_revision":{"type":"integer","minimum":0}}),
            &["expected_revision"],
        );
    }
    let value = match name {
        "assign_task" => object(
            json!({"run":reference("identity"),"parent":reference("identity"),"brief":reference("brief")}),
            &["run", "parent", "brief"],
        ),
        "freeze_task" | "cancel_task" | "observe_delivery" => reference("identity"),
        "publish_result" => object(
            json!({"task":reference("identity"),"result":reference("result")}),
            &["task", "result"],
        ),
        "merge_task" => object(
            json!({"task":reference("identity"),"target":reference("binding"),"result":reference("object_ref")}),
            &["task", "target", "result"],
        ),
        "answer_question" => object(
            json!({"question":reference("identity"),"answer":reference("object_ref")}),
            &["question", "answer"],
        ),
        "send_message" => object(
            json!({"command_id":reference("command_id"),"run_id":reference("identity"),"expected_control_revision":reference("identity"),"action":object(json!({"kind":{"const":"send_message"},"text_ref":reference("object_ref")}),&["kind","text_ref"])}),
            &[
                "command_id",
                "run_id",
                "expected_control_revision",
                "action",
            ],
        ),
        _ => unreachable!("internal capability name"),
    };
    let kind = if name == "send_message" {
        "control_run"
    } else {
        name
    };
    let mut schema = object(
        json!({"id":reference("command_id"),"expected_generation":{"type":"integer","minimum":0},"owner_epoch":{"type":"integer","minimum":1},"expected_head":reference("identity"),"command":object(json!({"kind":{"const":kind},"value":value}),&["kind","value"])}),
        &[
            "id",
            "expected_generation",
            "owner_epoch",
            "expected_head",
            "command",
        ],
    );
    schema["$defs"] = definitions();
    schema
}
