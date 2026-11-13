// Smoke test the standalone renderer's public API: prepare a PBF, render the
// fit-zoom center tile as a base64 data URL, decode it, and write the PNG.
use osm_renderer::map;

fn main() {
    let pbf = std::env::args().nth(1).expect("usage: render <pbf> <region> [out.png]");
    let region = std::env::args().nth(2).unwrap_or_else(|| "region".into());
    let out = std::env::args().nth(3).unwrap_or_else(|| "/tmp/tile.png".into());

    let meta = map::MapMeta {
        region: region.clone(),
        version: 0,
        md5_hex: String::new(),
        cid: None,
    };
    let prep = map::prepare(std::path::Path::new(&pbf), meta).expect("prepare");
    let info = &prep["info"];
    println!(
        "built: ways={} nodes={} fit_zoom={}",
        info["ways"], info["nodes"], info["fit_zoom"]
    );
    let z = info["fit_zoom"].as_u64().unwrap_or(6) as u32;
    let b = &info["bounds"];
    let clat = b["center_lat"].as_f64().unwrap_or(0.0);
    let clon = b["center_lon"].as_f64().unwrap_or(0.0);
    let n = 1u32 << z;
    let tx = (((clon + 180.0) / 360.0 * n as f64).floor() as u32).min(n - 1);
    let latr = clat.to_radians();
    let ty = (((1.0 - latr.tan().asinh() / std::f64::consts::PI) / 2.0 * n as f64).floor() as u32).min(n - 1);
    let png = map::render_tile_png(z as u8, tx, ty);
    std::fs::write(&out, &png).unwrap();
    println!("wrote {out} ({} bytes) tile z={z} x={tx} y={ty}", png.len());
    assert!(png.len() > 100 && &png[..8] == b"\x89PNG\r\n\x1a\n", "not a PNG");
    println!("OK: valid PNG from public API");
    map::stop();
}
