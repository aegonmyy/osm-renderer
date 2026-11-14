//! `osm_renderer` core module (Rust cdylib, `interface: cdylib`).
//!
//! The renderer: parse an OSM region `.osm.pbf` into a z9-tile-bucketed spatial
//! index, then rasterize 256×256 Web-Mercator PNG tiles on demand. The
//! `osm_renderer_app` (ui_qml) fetches the PBF from Logos Storage by CID via the
//! `osm` module, hands the local path to `map_prepare`, polls `map_status`
//! until the index is built, then calls `map_render_tile` per visible tile and
//! hands the returned base64 data URL to the host's SandboxImages capability
//! (the Basecamp QML sandbox blocks http/file image sources).
//!
//! The IPC contract is `osm_renderer.lidl` — a single generic `invoke_op(name,
//! args)` the app plugin forwards `map_*` ops through, mirroring how `osm_app`
//! drives the `osm` core's `invokeOpJson`. The builder generates the trait +
//! `install` scaffold (`generated/provider_gen.rs`) from the .lidl. Every op
//! returns a JSON string the QML parses.

include!(concat!(env!("CARGO_MANIFEST_DIR"), "/generated/provider_gen.rs"));

mod map;

use serde_json::{json, Value};

/// base64-encode a PNG so the QML can wrap it as `data:image/png;base64,…`.
fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push(T[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(T[((n >> 6) & 63) as usize] as char);
        out.push('=');
    }
    out
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(|s| s.to_string())
}
fn i_arg(args: &Value, key: &str) -> Option<i64> {
    args.get(key)?.as_i64()
}

/// Dispatch a renderer op. Mirrors the osm core's `{"ok":…,"result":…}` /
/// `{"ok":false,"error":…}` contract, tagged with the op name for QML routing.
fn dispatch(name: &str, args: &Value) -> Value {
    match name {
        "map_prepare" => {
            let Some(pbf) = str_arg(args, "pbf") else {
                return json!({ "ok": false, "error": "map_prepare requires pbf" });
            };
            let region = str_arg(args, "region").unwrap_or_else(|| "region".into());
            let meta = map::MapMeta {
                region,
                version: i_arg(args, "version").unwrap_or(0).max(0) as u32,
                md5_hex: str_arg(args, "md5").unwrap_or_default(),
                cid: str_arg(args, "cid").filter(|s| !s.is_empty()),
            };
            match map::prepare(std::path::Path::new(&pbf), meta) {
                Ok(v) => json!({ "ok": true, "result": v }),
                Err(e) => json!({ "ok": false, "error": e }),
            }
        }
        "map_render_tile" => {
            let (Some(z), Some(x), Some(y)) =
                (i_arg(args, "z"), i_arg(args, "x"), i_arg(args, "y"))
            else {
                return json!({ "ok": false, "error": "map_render_tile requires z,x,y" });
            };
            if z < 0 || x < 0 || y < 0 {
                return json!({ "ok": false, "error": "z,x,y must be non-negative" });
            }
            let png = map::render_tile_png(z as u8, x as u32, y as u32);
            if png.is_empty() {
                return json!({ "ok": false, "error": "no map ready" });
            }
            json!({ "ok": true, "result": { "data_url": format!("data:image/png;base64,{}", b64(&png)) } })
        }
        "map_status" => json!({ "ok": true, "result": map::status() }),
        "map_stop" => {
            map::stop();
            json!({ "ok": true, "result": { "stopped": true } })
        }
        other => json!({ "ok": false, "error": format!("unknown op: {other}") }),
    }
}

#[derive(Default)]
struct RendererImpl;

impl OsmRendererCoreModule for RendererImpl {
    /// Generic op dispatch: `name` is one of map_prepare/map_render_tile/
    /// map_status/map_stop; `args` is a JSON object string. Returns a JSON
    /// result string tagged with the op name.
    fn invoke_op(&mut self, name: String, args: String) -> String {
        let args: Value = serde_json::from_str(&args).unwrap_or_else(|_| Value::Object(Default::default()));
        let mut v = dispatch(&name, &args);
        if let Value::Object(ref mut m) = v {
            m.insert("op".into(), json!(name));
        }
        v.to_string()
    }
}

#[no_mangle]
pub extern "Rust" fn logos_module_install() {
    install::<RendererImpl>();
}
