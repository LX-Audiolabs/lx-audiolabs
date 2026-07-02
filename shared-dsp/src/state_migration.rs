// nice-plug → truce State Migration
//
// nice-plug serialized plugin state as JSON:
//   {"version": X, "params": {"param_name": value, ...}}
//
// truce uses a binary envelope with magic bytes. When a DAW session saved
// with a nice-plug build is loaded by the truce build, the binary parser
// rejects the JSON blob. The host then falls back to defaults — silent
// data loss for users.
//
// This module provides a fallback: detect nice-plug JSON in load_state(),
// parse it, extract parameter values, and write them back through the
// normal truce param API. Old sessions "just work".

use serde_json::Value;

/// Try to parse a nice-plug JSON state blob.
///
/// Returns `Some(params)` if the data starts with `{` and parses as a
/// nice-plug state object with a `"params"` key. Returns `None` if the
/// data does not look like nice-plug JSON (truce binary or empty).
///
/// The returned map has parameter names (display names like "Low Gain")
/// as keys and raw values (not 0.0–1.0 normalized) as `f64`.
pub fn try_parse_niceplug_state(data: &[u8]) -> Option<Vec<(String, f64)>> {
    // Quick check: if it doesn't start with '{', it can't be nice-plug JSON.
    if data.first() != Some(&b'{') {
        return None;
    }

    let root: Value = serde_json::from_slice(data).ok()?;
    let params = root.get("params")?.as_object()?;

    let mut result = Vec::with_capacity(params.len());
    for (name, val) in params {
        if let Some(v) = val.as_f64() {
            result.push((name.clone(), v));
        }
        // ponytail: non-f64 values (bool, int) — nice-plug encoded them
        // as floats too (0.0 / 1.0 for bools), so we skip anything else.
    }

    Some(result)
}
