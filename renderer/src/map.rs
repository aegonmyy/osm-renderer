//! On-demand raster map renderer for a region PBF fetched by CID.
//!
//! The consumer story is "fetch the region's PBF from Logos Storage by its
//! on-chain CID, then look at it" — but a `.osm.pbf` is not something a person
//! can read. This module turns a local PBF into a pannable, zoomable map with
//! no external tile toolchain on the user's machine and no WebEngine: a
//! [`MapIndex`] is built once from the PBF (renderable ways + their resolved
//! node coordinates, bucketed by z9 Web-Mercator tile), and an embedded HTTP
//! server renders 256×256 raster PNG tiles on demand from that index. A
//! pure-QtQuick slippy-map view fetches `http://127.0.0.1:<port>/tiles/z/x/y.png`.
//!
//! Scope (v1): roads, water (bodies + ways), landuse/parks, administrative
//! boundaries — enough that a region is recognizably a map. Text labels and
//! multi-relation coastlines are deferred; the shape reads as a map without
//! them.
//!
//! The renderer lives in the SDK core (the `osm` module's cdylib) so it ships
//! cross-platform with `liblogos_osm` and runs in the same host process that
//! already holds the imported PBF on disk.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{json, Value};

/// Quantized coordinate: lat/lon in units of 1e-7 degrees (~1 cm), packed as
/// two i32s. Storing ways as `Vec<(i32, i32)>` instead of `Vec<(f64, f64)>`
/// roughly halves the index's resident memory, and 1 cm is far finer than any
/// tile pixel ever resolves.
#[derive(Clone, Copy, Debug)]
struct Pt {
    lat_e7: i32,
    lon_e7: i32,
}

impl Pt {
    fn lat(&self) -> f64 {
        self.lat_e7 as f64 * 1e-7
    }
    fn lon(&self) -> f64 {
        self.lon_e7 as f64 * 1e-7
    }
}

/// What a renderable way is, for styling. Order matters for draw order
/// (later variants draw on top of earlier ones in [`render_tile`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Landuse,    // closed area: residential, industrial, farmland, etc.
    Park,       // leisure=park / landuse=forest / natural=wood
    WaterBody,  // closed natural=water / landuse=reservoir
    Boundary,   // administrative boundary (line)
    Waterway,   // river/stream/canal (line)
    Highway,    // road; sub-class drives width/color
}

/// Road subclass for the `Highway` kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Road {
    Minor,       // service, residential, unclassified, footway, path...
    Secondary,   // secondary, tertiary
    Primary,     // primary, trunk
    Motorway,    // motorway
}

/// One renderable way: its kind (and road class if a highway), whether it is a
/// closed ring (an area), and its resolved points. Stored once; z9 tile buckets
/// hold indices into the [`MapIndex::ways`] vec to avoid duplicating geometry
/// across the buckets a way spans.
#[derive(Clone, Debug)]
struct Way {
    kind: Kind,
    road: Option<Road>,
    closed: bool,
    points: Vec<Pt>,
}

/// A built map index, ready to render tiles from.
pub struct MapIndex {
    region: String,
    version: u32,
    md5_hex: String,
    cid: Option<String>,
    ways: Vec<Way>,
    /// z9 tile (x,y) -> indices into `ways`. A way is filed under every z9 tile
    /// its bounding box touches so a tile request only scans the buckets it
    /// falls in, not the whole region.
    buckets: HashMap<(u32, u32), Vec<u32>>,
    /// Overall region bounds (min/max lat/lon) so the view can fit the region.
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
    way_count: usize,
    node_count: usize,
}

/// Global renderer state: the active index. One map at a time per module
/// process — a consumer views one region.
static MAP: OnceLock<Mutex<Option<Arc<MapIndex>>>> = OnceLock::new();

fn map_lock() -> &'static Mutex<Option<Arc<MapIndex>>> {
    MAP.get_or_init(|| Mutex::new(None))
}

// ---------------------------------------------------------------------------
// Projection (Web Mercator)
// ---------------------------------------------------------------------------

/// The standard slippy-map tile count at zoom `z`.
#[inline]
fn tile_count(z: u8) -> u32 {
    1u32 << z
}

/// Longitude to the continuous tile-x coordinate at zoom `z`.
#[inline]
fn lon_to_x(lon: f64, z: u8) -> f64 {
    (lon + 180.0) / 360.0 * tile_count(z) as f64
}

/// Latitude (degrees) to the continuous tile-y coordinate at zoom `z`, via the
/// Web Mercator forward projection.
#[inline]
fn lat_to_y(lat: f64, z: u8) -> f64 {
    let lat = lat.to_radians();
    // asinh(tan(lat)) = ln(tan(lat) + sec(lat)) — the Web Mercator forward y.
    let v = lat.tan().asinh();
    (1.0 - v / std::f64::consts::PI) / 2.0 * tile_count(z) as f64
}

/// Project a lat/lon to a pixel within tile (z, tx, ty), where the tile is
/// `size` pixels on a side. Returns None if the point is outside the vertical
/// Mercator domain (|lat| > ~85.05, where the projection blows up).
fn project(lat: f64, lon: f64, z: u8, tx: u32, ty: u32, size: f64) -> Option<(f64, f64)> {
    if lat.abs() >= 89.9 {
        return None;
    }
    let fx = (lon_to_x(lon, z) - tx as f64) * size;
    let fy = (lat_to_y(lat, z) - ty as f64) * size;
    if !fx.is_finite() || !fy.is_finite() {
        return None;
    }
    Some((fx, fy))
}

/// The z9 tile (x,y) that a lat/lon falls in.
fn z9_tile(lat: f64, lon: f64) -> (u32, u32) {
    let z = 9u8;
    let x = (lon_to_x(lon, z).floor() as i64).clamp(0, (tile_count(z) - 1) as i64) as u32;
    let y = (lat_to_y(lat, z).floor() as i64).clamp(0, (tile_count(z) - 1) as i64) as u32;
    (x, y)
}

// ---------------------------------------------------------------------------
// Index construction
// ---------------------------------------------------------------------------

/// Classify a way's tags into a renderable [`Kind`], or None to skip it.
fn classify(tags: &[(&str, &str)]) -> Option<(Kind, Option<Road>)> {
    // A way can carry several of these; pick the most visually meaningful.
    // Order: water body > waterway > park > landuse > boundary > highway —
    // reversed from draw order so a way is filed under its dominant feature.
    let mut water_body = false;
    let mut waterway = false;
    let mut park = false;
    let mut landuse = false;
    let mut boundary = false;
    let mut road: Option<Road> = None;
    for &(k, v) in tags {
        match k {
            "natural" => match v {
                "water" | "bay" | "strait" | "coastline" => water_body = true,
                "wood" | "scrub" | "grassland" | "heath" | "wetland" => park = true,
                _ => {}
            },
            "waterway" => {
                waterway = true;
            }
            "landuse" => match v {
                "reservoir" | "basin" => water_body = true,
                "forest" | "meadow" | "farmland" | "orchard" | "vineyard" | "greenhouse_horticulture"
                | "cemetery" | "recreation_ground" | "village_green" | "allotments" => park = true,
                _ => landuse = true,
            },
            "leisure" => match v {
                "park" | "garden" | "pitch" | "playground" | "nature_reserve" | "golf_course" | "common" => {
                    park = true
                }
                _ => {}
            },
            "boundary" => {
                if v == "administrative" {
                    boundary = true;
                }
            }
            "highway" => {
                road = Some(match v {
                    "motorway" | "motorway_link" | "trunk" | "trunk_link" => Road::Motorway,
                    "primary" | "primary_link" => Road::Primary,
                    "secondary" | "secondary_link" | "tertiary" | "tertiary_link" | "unclassified" => {
                        Road::Secondary
                    }
                    _ => Road::Minor, // residential, service, footway, path, track, ...
                });
            }
            _ => {}
        }
    }
    if water_body {
        return Some((Kind::WaterBody, None));
    }
    if waterway {
        return Some((Kind::Waterway, None));
    }
    if park {
        return Some((Kind::Park, None));
    }
    if landuse {
        return Some((Kind::Landuse, None));
    }
    if boundary {
        return Some((Kind::Boundary, None));
    }
    if road.is_some() {
        return Some((Kind::Highway, road));
    }
    None
}

impl MapIndex {
    /// Build a render index from a local PBF. `meta` carries the provenance the
    /// view shows alongside the map (region, version, MD5, CID).
    pub fn build(pbf: &Path, meta: MapMeta) -> anyhow::Result<Self> {
        let mut reader = osmpbf::IndexedReader::from_path(pbf)
            .map_err(|e| anyhow::anyhow!("opening pbf: {e}"))?;

        // read_ways_and_deps makes two passes: first it emits every matching
        // Way (we record its tags + node refs), then it emits every Node those
        // ways reference (we record id -> coord). All ways arrive before all
        // nodes, so two independent accumulators are enough — no pass tracking.
        struct WayRec {
            kind: Kind,
            road: Option<Road>,
            refs: Vec<i64>,
        }
        let way_recs: Mutex<Vec<WayRec>> = Mutex::new(Vec::new());
        let nodes: Mutex<HashMap<i64, Pt>> = Mutex::new(HashMap::new());

        reader
            .read_ways_and_deps(
                |way| {
                    let t: Vec<(&str, &str)> = way.tags().collect();
                    classify(&t).is_some()
                },
                |element| match element {
                    osmpbf::Element::Way(way) => {
                        let t: Vec<(&str, &str)> = way.tags().collect();
                        if let Some((kind, road)) = classify(&t) {
                            let refs: Vec<i64> = way.refs().collect();
                            way_recs.lock().unwrap().push(WayRec { kind, road, refs });
                        }
                    }
                    osmpbf::Element::Node(node) => {
                        nodes.lock().unwrap().insert(
                            node.id(),
                            Pt {
                                lat_e7: (node.lat() * 1e7).round() as i32,
                                lon_e7: (node.lon() * 1e7).round() as i32,
                            },
                        );
                    }
                    osmpbf::Element::DenseNode(node) => {
                        nodes.lock().unwrap().insert(
                            node.id,
                            Pt {
                                lat_e7: (node.lat() * 1e7).round() as i32,
                                lon_e7: (node.lon() * 1e7).round() as i32,
                            },
                        );
                    }
                    osmpbf::Element::Relation(_) => {}
                },
            )
            .map_err(|e| anyhow::anyhow!("reading pbf: {e}"))?;

        let way_recs = way_recs.into_inner().unwrap();
        let nodes = nodes.into_inner().unwrap();
        let node_count = nodes.len();
        let way_count = way_recs.len();

        // Resolve each way's points, drop ways with unresolved geometry, file
        // each under the z9 tiles its bbox touches, and accumulate region bounds.
        let mut ways: Vec<Way> = Vec::with_capacity(way_recs.len());
        let mut buckets: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
        let mut min_lat = f64::INFINITY;
        let mut max_lat = f64::NEG_INFINITY;
        let mut min_lon = f64::INFINITY;
        let mut max_lon = f64::NEG_INFINITY;

        for rec in way_recs {
            if rec.refs.len() < 2 {
                continue;
            }
            let mut points = Vec::with_capacity(rec.refs.len());
            let mut w_min_lat = f64::INFINITY;
            let mut w_max_lat = f64::NEG_INFINITY;
            let mut w_min_lon = f64::INFINITY;
            let mut w_max_lon = f64::NEG_INFINITY;
            for r in &rec.refs {
                let Some(pt) = nodes.get(r) else {
                    // A referenced node not found in the file (truncated extract
                    // or a way crossing the extract border) — skip the whole
                    // way rather than emit a gap, so lines don't tear.
                    continue;
                };
                let lat = pt.lat();
                let lon = pt.lon();
                points.push(*pt);
                w_min_lat = w_min_lat.min(lat);
                w_max_lat = w_max_lat.max(lat);
                w_min_lon = w_min_lon.min(lon);
                w_max_lon = w_max_lon.max(lon);
            }
            if points.len() < 2 {
                continue;
            }
            let closed = rec.refs.first() == rec.refs.last() && points.len() >= 4;
            let idx = ways.len() as u32;
            ways.push(Way {
                kind: rec.kind,
                road: rec.road,
                closed,
                points,
            });
            // Region bounds.
            min_lat = min_lat.min(w_min_lat);
            max_lat = max_lat.max(w_max_lat);
            min_lon = min_lon.min(w_min_lon);
            max_lon = max_lon.max(w_max_lon);
            // File under every z9 tile the way's bbox touches.
            let (tx0, ty0) = z9_tile(w_min_lat, w_min_lon);
            let (tx1, ty1) = z9_tile(w_max_lat, w_max_lon);
            let (tx0, tx1) = (tx0.min(tx1), tx0.max(tx1));
            let (ty0, ty1) = (ty0.min(ty1), ty0.max(ty1));
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    buckets.entry((tx, ty)).or_default().push(idx);
                }
            }
        }

        if ways.is_empty() || !min_lat.is_finite() {
            return Err(anyhow::anyhow!(
                "no renderable ways with resolved geometry found in {}",
                pbf.display()
            ));
        }

        Ok(MapIndex {
            region: meta.region,
            version: meta.version,
            md5_hex: meta.md5_hex,
            cid: meta.cid,
            ways,
            buckets,
            min_lat,
            max_lat,
            min_lon,
            max_lon,
            way_count,
            node_count,
        })
    }

    /// The bounds + provenance the view needs to fit and label the map.
    fn info(&self) -> Value {
        // Fit zoom: the whole-integral zoom at which the region's longer side
        // fits in a 256×256 tile, then one deeper so it isn't a single dot.
        let span_lat = self.max_lat - self.min_lat;
        let span_lon = self.max_lon - self.min_lon;
        let center_lat = (self.min_lat + self.max_lat) / 2.0;
        let center_lon = (self.min_lon + self.max_lon) / 2.0;
        // Span in tile units at zoom z is span_lon/360*2^z (lon) and the
        // Mercator y-span. Pick the largest z where both spans <= 1 tile.
        let mut fit = 2u8;
        for z in 2..=18 {
            let sx = lon_to_x(self.max_lon, z) - lon_to_x(self.min_lon, z);
            let sy = lat_to_y(self.min_lat, z) - lat_to_y(self.max_lat, z);
            if sx <= 1.0 && sy <= 1.0 {
                fit = z;
            } else {
                break;
            }
        }
        json!({
            "region": self.region,
            "version": self.version,
            "md5": self.md5_hex,
            "cid": self.cid,
            "bounds": {
                "min_lat": self.min_lat, "max_lat": self.max_lat,
                "min_lon": self.min_lon, "max_lon": self.max_lon,
                "center_lat": center_lat, "center_lon": center_lon,
                "span_lat": span_lat, "span_lon": span_lon,
            },
            "fit_zoom": fit,
            "min_zoom": 4,
            "max_zoom": 16,
            "ways": self.way_count,
            "nodes": self.node_count,
        })
    }

    /// The set of z9 buckets a requested tile (z,x,y) draws from. For z>=9 the
    /// requested tile sits inside a single z9 tile; for z<9 it spans a block of
    /// them.
    fn covering_buckets(&self, z: u8, x: u32, y: u32) -> Vec<(u32, u32)> {
        if z >= 9 {
            let s = (z - 9) as u32;
            vec![(
                (x >> s).min(tile_count(9) - 1),
                (y >> s).min(tile_count(9) - 1),
            )]
        } else {
            let s = 9 - z;
            let tx0 = (x << s).min(tile_count(9) - 1);
            let ty0 = (y << s).min(tile_count(9) - 1);
            let span = 1u32 << s;
            let tx1 = (tx0.saturating_add(span - 1)).min(tile_count(9) - 1);
            let ty1 = (ty0.saturating_add(span - 1)).min(tile_count(9) - 1);
            let mut out = Vec::new();
            let mut ty = ty0;
            while ty <= ty1 {
                let mut tx = tx0;
                while tx <= tx1 {
                    out.push((tx, ty));
                    tx += 1;
                }
                ty += 1;
            }
            out
        }
    }

    /// Render one 256×256 PNG tile.
    fn render_png(&self, z: u8, x: u32, y: u32) -> Vec<u8> {
        const S: u32 = 256;
        let mut img = image::ImageBuffer::new(S, S);
        // Base land fill.
        for px in img.pixels_mut() {
            *px = image::Rgb([24, 27, 33]);
        }

        if z < 4 || z > 16 || x >= tile_count(z) || y >= tile_count(z) {
            return encode_png(&img);
        }

        let size = S as f64;
        // Collect candidate way indices from the covering z9 buckets, dedup so
        // a way filed under several buckets in the block is drawn once.
        let mut seen: Vec<bool> = vec![false; self.ways.len()];
        for (bx, by) in self.covering_buckets(z, x, y) {
            if let Some(ids) = self.buckets.get(&(bx, by)) {
                for &id in ids {
                    if (id as usize) < seen.len() {
                        seen[id as usize] = true;
                    }
                }
            }
        }
        // Unique indices preserving first-seen order.
        let mut uniq: Vec<u32> = Vec::new();
        for (i, s) in seen.iter().enumerate() {
            if *s {
                uniq.push(i as u32);
            }
        }

        // Layer passes, in back-to-front order.
        for layer in [
            Kind::Landuse,
            Kind::Park,
            Kind::WaterBody,
            Kind::Boundary,
            Kind::Waterway,
            Kind::Highway,
        ] {
            for &id in &uniq {
                let w = &self.ways[id as usize];
                if w.kind != layer {
                    continue;
                }
                draw_way(&mut img, w, z, x, y, size);
            }
        }

        encode_png(&img)
    }
}

/// Provenance carried into the index for the view's header.
pub struct MapMeta {
    pub region: String,
    pub version: u32,
    pub md5_hex: String,
    pub cid: Option<String>,
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

#[inline]
fn px(img: &mut image::ImageBuffer<image::Rgb<u8>, Vec<u8>>, x: i32, y: i32, c: image::Rgb<u8>) {
    if x >= 0 && (x as u32) < img.width() && y >= 0 && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

/// Draw a thick-ish point: a square of half-width `r` (cheap road casing).
fn blob(img: &mut image::ImageBuffer<image::Rgb<u8>, Vec<u8>>, x: i32, y: i32, r: i32, c: image::Rgb<u8>) {
    for dy in -r..=r {
        for dx in -r..=r {
            px(img, x + dx, y + dy, c);
        }
    }
}

/// Bresenham line between two pixel points.
fn line(img: &mut image::ImageBuffer<image::Rgb<u8>, Vec<u8>>, x0: i32, y0: i32, x1: i32, y1: i32, c: image::Rgb<u8>) {
    let (mut x0, mut y0) = (x0, y0);
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        px(img, x0, y0, c);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Thick line by stamping a small square along the Bresenham path.
fn thick_line(
    img: &mut image::ImageBuffer<image::Rgb<u8>, Vec<u8>>,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    w: i32,
    c: image::Rgb<u8>,
) {
    if w <= 0 {
        line(img, x0, y0, x1, y1, c);
        return;
    }
    let (mut x, mut y) = (x0, y0);
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        blob(img, x, y, w, c);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Scanline-fill a polygon given pixel-space points (assumes a simple ring).
fn fill_polygon(
    img: &mut image::ImageBuffer<image::Rgb<u8>, Vec<u8>>,
    pts: &[(f64, f64)],
    c: image::Rgb<u8>,
) {
    if pts.len() < 3 {
        return;
    }
    let h = img.height() as i32;
    let w = img.width() as i32;
    let mut ymin = h;
    let mut ymax = -1;
    for (_, y) in pts {
        let yi = y.round() as i32;
        ymin = ymin.min(yi);
        ymax = ymax.max(yi);
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    let n = pts.len();
    for scan in ymin..=ymax {
        let yf = scan as f64 + 0.5;
        let mut xs = Vec::new();
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = pts[i];
            let (xj, yj) = pts[j];
            if (yi <= yf) != (yj <= yf) {
                let dy = yj - yi;
                if dy != 0.0 {
                    let t = (yf - yi) / dy;
                    let x = xi + t * (xj - xi);
                    xs.push(x);
                }
            }
            j = i;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut k = 0;
        while k + 1 < xs.len() {
            let x0 = xs[k].round() as i32;
            let x1 = xs[k + 1].round() as i32;
            for x in x0.max(0)..=x1.min(w - 1) {
                img.put_pixel(x as u32, scan as u32, c);
            }
            k += 2;
        }
    }
}

fn draw_way(
    img: &mut image::ImageBuffer<image::Rgb<u8>, Vec<u8>>,
    w: &Way,
    z: u8,
    tx: u32,
    ty: u32,
    size: f64,
) {
    // Project points; clip the way if none of its points land in the tile's
    // neighborhood — we still draw lines between consecutive projected points,
    // which naturally handles ways crossing the tile (the line math clips at
    // the pixel boundaries via `px`).
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(w.points.len());
    for p in &w.points {
        if let Some((fx, fy)) = project(p.lat(), p.lon(), z, tx, ty, size) {
            pts.push((fx, fy));
        }
    }
    if pts.len() < 2 {
        return;
    }

    match w.kind {
        Kind::Landuse if w.closed => {
            fill_polygon(img, &pts, image::Rgb([34, 38, 46]));
        }
        Kind::Park if w.closed => {
            fill_polygon(img, &pts, image::Rgb([31, 45, 33]));
        }
        Kind::WaterBody if w.closed => {
            fill_polygon(img, &pts, image::Rgb([21, 57, 74]));
        }
        Kind::Boundary => {
            // Thin line, administrative boundary.
            for win in pts.windows(2) {
                line(
                    img,
                    win[0].0.round() as i32,
                    win[0].1.round() as i32,
                    win[1].0.round() as i32,
                    win[1].1.round() as i32,
                    image::Rgb([58, 74, 58]),
                );
            }
        }
        Kind::Waterway => {
            // Rivers: a modest blue line that thickens with zoom.
            let width = if z >= 12 { 2 } else { 1 };
            for win in pts.windows(2) {
                thick_line(
                    img,
                    win[0].0.round() as i32,
                    win[0].1.round() as i32,
                    win[1].0.round() as i32,
                    win[1].1.round() as i32,
                    width,
                    image::Rgb([42, 111, 143]),
                );
            }
        }
        Kind::Highway => {
            let (width, color) = match w.road.unwrap_or(Road::Minor) {
                Road::Motorway => (match z {
                    0..=8 => 0,
                    9..=11 => 1,
                    _ => 2,
                }, image::Rgb([224, 164, 75])),
                Road::Primary => (match z {
                    0..=9 => 0,
                    10..=12 => 1,
                    _ => 2,
                }, image::Rgb([201, 122, 58])),
                Road::Secondary => (match z {
                    0..=10 => 0,
                    11..=13 => 1,
                    _ => 2,
                }, image::Rgb([138, 143, 152])),
                Road::Minor => (match z {
                    0..=11 => 0,
                    12..=14 => 0,
                    _ => 1,
                }, image::Rgb([90, 96, 104])),
            };
            // width 0 at low zoom still draws a 1px line for the road network
            // to read as a network; only truly suppress nothing.
            let w_px = if width == 0 { 0 } else { width };
            for win in pts.windows(2) {
                if w_px == 0 {
                    line(
                        img,
                        win[0].0.round() as i32,
                        win[0].1.round() as i32,
                        win[1].0.round() as i32,
                        win[1].1.round() as i32,
                        color,
                    );
                } else {
                    thick_line(
                        img,
                        win[0].0.round() as i32,
                        win[0].1.round() as i32,
                        win[1].0.round() as i32,
                        win[1].1.round() as i32,
                        w_px,
                        color,
                    );
                }
            }
        }
        // Closed ways of line-kind, or open ways of area-kind: fall back to line.
        _ => {
            for win in pts.windows(2) {
                line(
                    img,
                    win[0].0.round() as i32,
                    win[0].1.round() as i32,
                    win[1].0.round() as i32,
                    win[1].1.round() as i32,
                    image::Rgb([60, 66, 74]),
                );
            }
        }
    }
}

fn encode_png(img: &image::ImageBuffer<image::Rgb<u8>, Vec<u8>>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096);
    let _ = image::DynamicImage::ImageRgb8(img.clone())
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png);
    buf
}

// ---------------------------------------------------------------------------
// FFI-facing ops (called from lib.rs dispatch)
// ---------------------------------------------------------------------------

/// `map_prepare` op: build the index from a local PBF and store it as the
/// active map. Returns the map info (bounds, fit zoom, provenance). The caller
/// runs this as a background job when the region is large (building parses the
/// whole PBF).
pub fn prepare(pbf: &Path, meta: MapMeta) -> Result<Value, String> {
    let index = Arc::new(MapIndex::build(pbf, meta).map_err(|e| format!("{e:#}"))?);
    let info = index.info();
    stop();
    *map_lock().lock().unwrap() = Some(index);
    Ok(json!({ "info": info }))
}

/// `map_render_tile` op: render one 256×256 PNG tile from the active index.
/// Returns the raw PNG bytes; the FFI layer base64-encodes it into a data URL.
pub fn render_tile_png(z: u8, x: u32, y: u32) -> Vec<u8> {
    let g = map_lock().lock().unwrap();
    match g.as_ref() {
        Some(idx) => idx.render_png(z, x, y),
        None => Vec::new(),
    }
}

/// `map_status` op: is a map loaded, and its provenance.
pub fn status() -> Value {
    let g = map_lock().lock().unwrap();
    match g.as_ref() {
        Some(i) => json!({
            "loaded": true,
            "region": i.region,
            "version": i.version,
            "md5": i.md5_hex,
            "cid": i.cid,
            "ways": i.way_count,
            "nodes": i.node_count,
        }),
        None => json!({ "loaded": false }),
    }
}

/// `map_stop` op: drop the active index.
pub fn stop() {
    *map_lock().lock().unwrap() = None;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_is_sane() {
        // Origin tile at z0 covers the whole world; (0,0) is the top-left
        // corner of that tile.
        let (px, py) = project(0.0, 0.0, 0, 0, 0, 256.0).unwrap();
        assert!((px - 128.0).abs() < 1e-6);
        // Web Mercator y at lat 0 is the vertical midpoint.
        assert!((py - 128.0).abs() < 1e-6);
        // Increasing longitude moves right.
        let (rx, _) = project(0.0, 1.0, 0, 0, 0, 256.0).unwrap();
        assert!(rx > 128.0);
        // Increasing latitude moves up (smaller y).
        let (_, uy) = project(1.0, 0.0, 0, 0, 0, 256.0).unwrap();
        assert!(uy < 128.0);
    }

    #[test]
    fn z9_tile_clamps() {
        // Edge longitudes map to the boundary tiles.
        let (x, _) = z9_tile(0.0, -180.0);
        assert_eq!(x, 0);
        let (x, _) = z9_tile(0.0, 179.999);
        assert_eq!(x, tile_count(9) - 1);
    }

    #[test]
    fn classify_roads_and_water() {
        assert_eq!(
            classify(&[("highway", "motorway")]),
            Some((Kind::Highway, Some(Road::Motorway)))
        );
        assert_eq!(
            classify(&[("highway", "residential")]),
            Some((Kind::Highway, Some(Road::Minor)))
        );
        assert_eq!(
            classify(&[("natural", "water")]),
            Some((Kind::WaterBody, None))
        );
        assert_eq!(
            classify(&[("waterway", "river")]),
            Some((Kind::Waterway, None))
        );
        assert_eq!(
            classify(&[("landuse", "residential")]),
            Some((Kind::Landuse, None))
        );
        assert_eq!(
            classify(&[("name", "nowhere")]),
            None
        );
    }
}
