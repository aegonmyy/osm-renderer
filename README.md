# osm_renderer — "Map Viewer" for Logos Basecamp

A standalone Basecamp app that turns an OpenStreetMap region distributed on
Logos into an actual map a person can look at: pick a region, the app resolves
it on-chain, fetches the verified `.osm.pbf` from Logos Storage by CID, and
renders it as a pannable, zoomable slippy map — roads, rivers, parks, admin
boundaries — with the region's provenance (version, CID, MD5) in the header.

It is a consumer of the [`osm` SDK module][logos-osm] and changes nothing in
it: region resolution, on-chain lookup, Logos Storage fetch, and MD5
verification are all forwarded to the unchanged `osm` core over RemoteObjects.
The rendering itself is a small pure-Rust cdylib bundled inside this app.

## How it works

```
QML view (sandboxed, in the Basecamp shell)
  │  invokeOp("regions"/"lookup"/"fetch"/"catalog")      → forwarded to the osm core module
  │  invokeOp("map_prepare"/"map_render_tile"/...)       → local renderer cdylib
  ▼
osm_renderer_plugin (ui-host process)
  │  dlopens libosm_renderer.so, calls osm_renderer_invoke (C ABI)
  ▼
libosm_renderer (Rust: osmpbf + image)
     map_prepare      parse the fetched PBF (osmpbf IndexedReader, two passes,
                      bounded memory: only renderable ways + their nodes)
                      into a z9-tile-bucketed spatial index
     map_render_tile  rasterize one 256×256 PNG (Web Mercator) on demand and
                      return it as data:image/png;base64
  ▼
QML hands each tile's data URL to the host's SandboxImages capability and
binds the returned image://sandbox-media/<id> URL
```

The Basecamp QML sandbox blocks `http://` and `file://` image sources (network
is deny-all), so tiles cannot be served over a local HTTP server inside the
app — they flow as base64 through the module boundary and are registered with
`SandboxImages.sourceFor(...)`.

## Layout

| Path | What it is |
|---|---|
| `renderer/` | the Rust cdylib (`libosm_renderer`): PBF → spatial index → raster PNG tiles. Unit-tested; `cargo run -p osm-renderer --example render -- <pbf> <region>` smoke-renders a tile. |
| `app/` | the ui_qml Basecamp app: plugin (routes `map_*` to the cdylib, forwards osm ops to the core), QML slippy-map view, metadata, flake. |
| `build-ffi.sh` | build the cdylib locally (dev). |
| `tools/pack_lgx.c` | a tiny C packer that builds a valid `.lgx` from a variant dir using Basecamp's own `liblgx` — lets you produce an installable package on a box without nix (dev/CI-bridge). |
| `.github/workflows/release.yml` | nix CI: builds the `.lgx` bundles (linux-amd64, windows-x86_64) and publishes them on `osm_renderer-v*` tags. |

## Build

The `.lgx` (the only thing Basecamp installs) is built by nix — there is no
local packager, and swapping a `.so` into an installed module breaks its
integrity hash, so the app must always ship through the CI build:

```sh
cd app
nix build .#lgx-portable
```

Then install in Basecamp: Settings → Package Manager → Install Local Package →
pick the `.lgx` (or publish it to a module catalog and install from there).

For quick renderer iteration without Basecamp:

```sh
cargo test -p osm-renderer
cargo run -p osm-renderer --example render -- ~/osm-anchor/cache/china_henan-latest.osm.pbf china/henan
```

## Dependencies

- The `osm` module (from [aegonmyy/logos-osm][logos-osm]) must be installed;
  this app only renders what it fetches.
- Demo region: `china/henan` (small, hosted on Logos Storage, registered
  on-chain with 3 mirrors).

[logos-osm]: https://github.com/aegonmyy/logos-osm
