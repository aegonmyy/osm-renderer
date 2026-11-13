//! `osm-renderer` — standalone raster map renderer cdylib for the
//! `osm_renderer` ("Map Viewer") Basecamp app.
//!
//! Takes a region `.osm.pbf` (the bytes the `osm` SDK module fetches from Logos
//! Storage by CID), builds a lightweight spatial index of renderable ways
//! (roads / water / landuse / boundaries), and renders 256×256 raster PNG tiles
//! on demand. The app plugin dlopens this cdylib and reaches it through a small
//! C ABI ([`logos_osm_renderer_invoke`]) that mirrors the `osm` core's
//! `invokeOp` contract, so the QML view drives it the same way.
//!
//! Tiles come back as `data:image/png;base64,…` URLs because the Basecamp QML
//! sandbox blocks http/file image sources — the view hands the data URL to the
//! host's `SandboxImages` capability and gets an opaque `image://` URL back.
//!
//! The renderer is pure Rust (osmpbf + image) and cross-compiles to Windows so
//! the same cdylib ships for linux-amd64 and windows-x86_64.

pub mod map;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

use serde_json::{json, Value};

/// Return a heap JSON string the caller must free with
/// `osm_renderer_free_string`.
fn to_json(v: Value) -> *mut c_char {
    let s = CString::new(v.to_string()).unwrap_or_else(|_| CString::new("null").unwrap());
    s.into_raw()
}

fn ok(obj: Value) -> *mut c_char {
    to_json(json!({ "ok": true, "result": obj }))
}

fn err(msg: impl Into<String>) -> *mut c_char {
    to_json(json!({ "ok": false, "error": msg.into() }))
}

fn str_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(|s| s.to_string())
}

fn u_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}

/// Render one tile as a `data:image/png;base64,…` URL the QML view passes to
/// `SandboxImages.sourceFor`. Reads the active index built by `map_prepare`.
pub fn render_tile_data_url(z: u8, x: u32, y: u32) -> Value {
    let png = map::render_tile_png(z, x, y);
    let b64 = base64_encode(&png);
    json!({ "data_url": format!("data:image/png;base64,{b64}") })
}

// ---- base64 (no extra dep; the image crate doesn't ship an encoder) ----
fn base64_encode(input: &[u8]) -> String {
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

// ---------------------------------------------------------------------------
// C ABI — the Qt plugin dlopens this cdylib and resolves these symbols.
// ---------------------------------------------------------------------------

/// Free a string previously returned by `osm_renderer_invoke` / `_version`.
///
/// # Safety
/// `ptr` must be null or a pointer previously returned by this FFI.
#[no_mangle]
pub unsafe extern "C" fn osm_renderer_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    drop(CString::from_raw(ptr));
}

#[no_mangle]
pub extern "C" fn osm_renderer_version() -> *mut c_char {
    to_json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "name": env!("CARGO_PKG_NAME"),
    }))
}

/// Dispatch a renderer op. `name` and `args_json` are null-terminated UTF-8.
/// Returns a heap JSON string the caller frees with
/// `osm_renderer_free_string`.
///
/// Ops: `map_prepare` (build the index from a local PBF), `map_render_tile`
/// (one tile as a base64 data URL), `map_status`, `map_stop`.
///
/// # Safety
/// `name` must be valid null-terminated UTF-8; `args_json`, if non-null, must
/// be valid null-terminated UTF-8 that parses as a JSON object.
#[no_mangle]
pub unsafe extern "C" fn osm_renderer_invoke(
    name: *const c_char,
    args_json: *const c_char,
) -> *mut c_char {
    if name.is_null() {
        return err("null op name");
    }
    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return err("non-utf8 op name"),
    };
    let args: Value = if args_json.is_null() {
        Value::Object(Default::default())
    } else {
        match CStr::from_ptr(args_json).to_str() {
            Err(_) => return err("non-utf8 args"),
            Ok(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::Object(Default::default())),
        }
    };
    dispatch(&name, &args)
}

fn dispatch(name: &str, args: &Value) -> *mut c_char {
    let raw = dispatch_op(name, args);
    // Tag with the op name (same routing convention as the osm core).
    let owned = unsafe { CString::from_raw(raw) };
    let parsed: Value = serde_json::from_str(owned.to_str().unwrap_or("{}"))
        .unwrap_or_else(|_| json!({"ok": false, "error": "unparseable result"}));
    let mut tagged = parsed;
    if let Value::Object(ref mut m) = tagged {
        m.entry("op").or_insert(json!(name));
    }
    to_json(tagged)
}

fn dispatch_op(name: &str, args: &Value) -> *mut c_char {
    match name {
        "version" => osm_renderer_version(),
        "map_prepare" => {
            let Some(pbf) = str_arg(args, "pbf") else {
                return err("map_prepare requires pbf (local file path)");
            };
            let region = str_arg(args, "region").unwrap_or_else(|| "region".to_string());
            let meta = map::MapMeta {
                region: region.clone(),
                version: u_arg(args, "version").unwrap_or(0) as u32,
                md5_hex: str_arg(args, "md5").unwrap_or_default(),
                cid: str_arg(args, "cid"),
            };
            match map::prepare(&std::path::PathBuf::from(pbf), meta) {
                Ok(v) => ok(v),
                Err(e) => err(e),
            }
        }
        "map_render_tile" => {
            let Some(z) = u_arg(args, "z") else { return err("map_render_tile requires z") };
            let Some(x) = u_arg(args, "x") else { return err("map_render_tile requires x") };
            let Some(y) = u_arg(args, "y") else { return err("map_render_tile requires y") };
            ok(render_tile_data_url(z as u8, x as u32, y as u32))
        }
        "map_status" => ok(map::status()),
        "map_stop" => {
            map::stop();
            ok(json!({ "stopped": true }))
        }
        other => err(format!("unknown op: {other}")),
    }
}
