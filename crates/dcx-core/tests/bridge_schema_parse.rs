//! Assert that both checked-in Logic bridge schemas parse as JSON objects
//! that declare a `$schema` dialect.
//!
//! This is the Bazel and Cargo form of `just schema-check`, which runs
//! `jq -e .` over the same two files. The `$schema` assertion is stricter than
//! jq. Both files are read at compile time, so Bazel declares them through
//! `compile_data`.

const BRIDGE_V1: &str = include_str!("../../../schemas/v1/dcx_logic_bridge.schema.json");
const BRIDGE_V2: &str = include_str!("../../../schemas/v2/dcx_logic_bridge.schema.json");

fn assert_schema_object(name: &str, text: &str) {
    let value: serde_json::Value = serde_json::from_str(text)
        .unwrap_or_else(|error| panic!("{name} is not valid JSON: {error}"));
    let object = value
        .as_object()
        .unwrap_or_else(|| panic!("{name} must be a JSON object"));
    assert!(
        object.contains_key("$schema"),
        "{name} must declare its JSON Schema dialect"
    );
}

#[test]
fn bridge_v1_schema_parses() {
    assert_schema_object("schemas/v1/dcx_logic_bridge.schema.json", BRIDGE_V1);
}

#[test]
fn bridge_v2_schema_parses() {
    assert_schema_object("schemas/v2/dcx_logic_bridge.schema.json", BRIDGE_V2);
}
