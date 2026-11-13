// osm_renderer — "Map Viewer". A pannable/zoomable slippy map of an OSM region
// fetched from Logos Storage by its on-chain CID.
//
// The renderer (PBF parse -> spatial index -> raster PNG) lives in
// libosm_renderer, dlopened by this app's plugin. Tiles come back as
// data:image/png;base64 URLs because the Basecamp QML sandbox blocks http and
// file image sources — each tile is handed to the host's SandboxImages
// capability, which returns an opaque image://sandbox-media/<id> URL only this
// engine's image provider can resolve.
//
// The plugin forwards osm ops (fetch/catalog/lookup) to the osm core module,
// and handles map_* ops locally against the renderer cdylib.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: root
    color: "#0b1512"

    readonly property var bk: logos.module("osm_renderer")
    property string lastErr: ""
    property var regionList: []
    property string mapState: "idle"   // idle | loading-list | preparing | ready | error
    property string mapError: ""
    property string prepareJobId: ""
    property var mapInfo: ({})
    property string mapRegion: ""
    property string mapPbf: ""

    // View state (the slippy map).
    property real centerLat: 0.0
    property real centerLon: 0.0
    property int  zoom: 4
    property int  minZoom: 4
    property int  maxZoom: 16
    property int  viewTick: 0
    readonly property int tileSize: 256

    function core(op, args) {
        if (!root.bk) return { ok: false, error: "no backend" }
        const raw = root.bk.invokeOp(op, JSON.stringify(args || {}))
        try { return JSON.parse(raw) } catch (e) { return { ok: false, error: String(raw) } }
    }

    // ---- Web Mercator helpers (mirror the Rust renderer) ----
    function pow2(z) { return Math.pow(2, z) }
    function lonToX(lon, z) { return (lon + 180.0) / 360.0 * pow2(z) }
    function latToY(lat, z) {
        const r = lat * Math.PI / 180.0
        const v = Math.log(Math.tan(r) + 1.0 / Math.cos(r))
        return (1.0 - v / Math.PI) / 2.0 * pow2(z)
    }
    function xToLon(x, z) { return x / pow2(z) * 360.0 - 180.0 }
    function yToLat(y, z) {
        const n = Math.PI - 2.0 * Math.PI * y / pow2(z)
        return 180.0 / Math.PI * Math.atan(Math.sinh(n))
    }

    function loadRegions() {
        root.mapState = "loading-list"
        const r = root.core("regions", {})
        if (r.ok && r.result && r.result.regions) {
            root.regionList = r.result.regions.map(g => ({ path: g.path, name: g.name }))
        } else {
            root.regionList = []
        }
        // Default to china/henan if present.
        if (root.regionList.length > 0) {
            const def = root.regionList.find(g => g.path === "china/henan")
            regionPicker.currentIndex = root.regionList.indexOf(def || root.regionList[0])
        }
        root.mapState = root.regionList.length > 0 ? "idle" : "error"
        if (root.regionList.length === 0) root.mapError = "no regions (is the osm module loaded?)"
    }

    // Resolve a region to a local PBF: look it up on-chain (cid/md5), then
    // fetch from Logos Storage, then find the local path in the catalog.
    function startMap(region) {
        root.mapRegion = region
        root.mapState = "preparing"
        root.mapError = ""
        root.prepareJobId = ""
        // 1. on-chain lookup for cid + md5.
        const lk = root.core("lookup", { region: region })
        if (!lk.ok || !lk.result || !lk.result.latest || !lk.result.latest.cid) {
            // No on-chain entry: fall back to the local catalog (a previously
            // imported PBF) so the map still works offline / pre-fetch.
            return root.prepareFromCatalog(region)
        }
        const latest = lk.result.latest
        const cid = latest.cid
        const md5 = latest.checksum || ""
        // 2. fetch from Logos Storage by CID (job: long op).
        const f = root.core("fetch", { region: region, cid: cid, md5_hex: md5, job: true })
        if (f.ok && f.result && f.result.job_id) {
            root.prepareJobId = f.result.job_id
            root.mapState = "preparing"
            root.mapError = "fetching region from Logos Storage…"
            fetchPoll.running = true
            fetchPoll.tick()
            return
        }
        // Fetch refused (already running?) or sync: try the catalog.
        root.prepareFromCatalog(region)
    }

    function prepareFromCatalog(region) {
        const cat = root.core("catalog", {})
        let pbf = ""
        if (cat.ok && cat.result) {
            const rec = cat.result.find(r => r.region === region)
            if (rec) pbf = rec.path || ""
        }
        if (!pbf) {
            root.mapState = "error"
            root.mapError = "no local PBF for '" + region + "'. Fetch it first."
            return
        }
        root.mapPbf = pbf
        root.runPrepare(region, pbf, cat)
    }

    function runPrepare(region, pbf, cat) {
        // Provenance from the catalog record.
        let version = 0, md5 = "", cid = ""
        if (cat.ok && cat.result) {
            const rec = cat.result.find(r => r.region === region)
            if (rec) { version = rec.version || 0; md5 = rec.md5 || ""; cid = rec.cid || "" }
        }
        const r = root.core("map_prepare", { pbf: pbf, region: region, version: version, md5: md5, cid: cid, job: true })
        if (!r.ok || !r.result || !r.result.job_id) {
            root.mapState = "error"
            root.mapError = (r.error || "map_prepare failed")
            return
        }
        root.prepareJobId = r.result.job_id
        root.mapState = "preparing"
        root.mapError = "building map index…"
        preparePoll.running = true
        preparePoll.tick()
    }

    // Poll a fetch job; on done, prepare the map from the fetched PBF.
    Timer {
        id: fetchPoll
        interval: 1000; repeat: true
        function tick() {
            if (!root.prepareJobId) return
            const r = root.core("job_status", { job_id: root.prepareJobId })
            if (!r.ok || !r.result) return
            const j = r.result
            if (j.state === "running") return
            fetchPoll.running = false
            root.prepareJobId = ""
            if (j.state === "done" && j.result && j.result.path) {
                root.mapPbf = j.result.path
                const cat = root.core("catalog", {})
                root.runPrepare(root.mapRegion, j.result.path, cat)
            } else {
                root.mapState = "error"
                root.mapError = j.error || "fetch failed"
            }
        }
        onTriggered: tick()
    }

    // Poll the map_prepare build job; on done, fit the view.
    Timer {
        id: preparePoll
        interval: 1000; repeat: true
        function tick() {
            if (!root.prepareJobId) return
            const r = root.core("job_status", { job_id: root.prepareJobId })
            if (!r.ok || !r.result) return
            const j = r.result
            if (j.state === "running") return
            preparePoll.running = false
            root.prepareJobId = ""
            if (j.state === "done" && j.result && j.result.info) {
                root.mapInfo = j.result.info
                const b = root.mapInfo.bounds || null
                if (b) {
                    root.centerLat = b.center_lat
                    root.centerLon = b.center_lon
                    root.zoom = Math.max(root.minZoom, Math.min(root.maxZoom,
                                Math.max(2, (root.mapInfo.fit_zoom || 6) + 1)))
                }
                root.mapState = "ready"
                root.viewTick++
            } else {
                root.mapState = "error"
                root.mapError = j.error || "build failed"
            }
        }
        onTriggered: tick()
    }

    function stopMap() {
        preparePoll.running = false; fetchPoll.running = false
        root.core("map_stop", {})
        root.mapState = "idle"
        root.mapInfo = ({})
    }

    Component.onCompleted: root.loadRegions()

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        // Header: title, region picker, render/stop, provenance.
        Rectangle {
            Layout.fillWidth: true; Layout.preferredHeight: 124
            color: Qt.rgba(0.886, 0.878, 0.788, 0.05)
            border.color: Qt.rgba(0.961, 0.961, 0.941, 0.14); border.width: 1

            RowLayout {
                anchors.fill: parent; anchors.margins: 14; spacing: 12

                ColumnLayout {
                    spacing: 6; Layout.fillWidth: true
                    Text { text: "MAP VIEWER"; font.family: "DejaVu Sans"; font.pixelSize: 11
                        color: Qt.rgba(0.961,0.961,0.941,0.55); font.letterSpacing: 2 }
                    Text { text: "OpenStreetMap, distributed on Logos"; font.family: "DejaVu Sans"
                        font.pixelSize: 16; color: "#f5f5ef" }
                    RowLayout {
                        spacing: 8
                        ComboBox {
                            id: regionPicker; Layout.preferredWidth: 260
                            model: root.regionList; textRole: "path"
                            font.family: "DejaVu Sans"
                            enabled: root.mapState !== "preparing"
                        }
                        Button {
                            text: root.mapState === "ready" ? "Re-render" : "Render map"
                            enabled: root.mapState !== "preparing"
                                       && regionPicker.currentValue !== undefined
                                       && String(regionPicker.currentValue).length > 0
                            onClicked: root.startMap(String(regionPicker.currentValue))
                            font.family: "DejaVu Sans"
                        }
                        Button {
                            text: "Stop"
                            enabled: root.mapState === "ready" || root.mapState === "preparing"
                            onClicked: root.stopMap()
                            font.family: "DejaVu Sans"
                        }
                    }
                }

                ColumnLayout {
                    spacing: 3; Layout.alignment: Qt.AlignTop | Qt.AlignRight
                    Text { text: root.mapState === "ready" && root.mapInfo.region
                              ? root.mapInfo.region : "—"
                        font.family: "DejaVu Sans Mono"; font.pixelSize: 13; color: "#ffd328" }
                    Text { visible: root.mapState === "ready"
                        text: "v" + (root.mapInfo.version || "—") + "  ·  z" + root.zoom
                              + "  ·  " + (root.mapInfo.ways || 0) + " ways"
                              + "  ·  " + (root.mapInfo.nodes || 0) + " nodes"
                        font.family: "DejaVu Sans Mono"; font.pixelSize: 11
                        color: Qt.rgba(0.961,0.961,0.941,0.55) }
                    Text { visible: root.mapState === "ready" && !!root.mapInfo.cid
                        text: "cid: " + String(root.mapInfo.cid || "").slice(0,24) + "…"
                        font.family: "DejaVu Sans Mono"; font.pixelSize: 11
                        color: Qt.rgba(0.961,0.961,0.941,0.45) }
                    Text { visible: root.mapState === "ready" && !!root.mapInfo.md5
                        text: "md5: " + String(root.mapInfo.md5 || "").slice(0,24) + "…"
                        font.family: "DejaVu Sans Mono"; font.pixelSize: 11
                        color: Qt.rgba(0.961,0.961,0.941,0.45) }
                    Text { visible: root.mapState === "preparing"
                        text: root.mapError || "working…"
                        font.family: "DejaVu Sans Mono"; font.pixelSize: 11; color: "#c9a23f"
                        wrapMode: Text.Wrap; Layout.maximumWidth: 360 }
                    Text { visible: root.mapState === "error"
                        text: root.mapError; font.family: "DejaVu Sans Mono"; font.pixelSize: 11
                        color: "#c0654e"; wrapMode: Text.Wrap; Layout.maximumWidth: 360 }
                }
            }
        }

        // The map viewport.
        Item {
            id: viewport
            Layout.fillWidth: true; Layout.fillHeight: true
            clip: true
            visible: root.mapState === "ready"
            property real vpW: width
            property real vpH: height

            function relayout() {
                if (viewport.vpW <= 0 || viewport.vpH <= 0) return
                const z = root.zoom
                const n = root.pow2(z)
                const ctx = root.lonToX(root.centerLon, z)
                const cty = root.latToY(root.centerLat, z)
                const cols = Math.ceil(viewport.vpW / root.tileSize) + 2
                const rows = Math.ceil(viewport.vpH / root.tileSize) + 2
                const halfC = Math.floor(cols / 2)
                const halfR = Math.floor(rows / 2)
                const ox = viewport.vpW / 2 - (ctx - Math.floor(ctx)) * root.tileSize - halfC * root.tileSize
                const oy = viewport.vpH / 2 - (cty - Math.floor(cty)) * root.tileSize - halfR * root.tileSize
                gridModel.clear()
                for (let dy = 0; dy < rows; dy++) {
                    for (let dx = 0; dx < cols; dx++) {
                        const tx = Math.floor(ctx) - halfC + dx
                        const ty = Math.floor(cty) - halfR + dy
                        const wx = ox + dx * root.tileSize
                        const wy = oy + dy * root.tileSize
                        const nx = ((tx % n) + n) % n
                        gridModel.append({ wx: wx, wy: wy, tx: nx, ty: ty, z: z, url: "" })
                    }
                }
                // Request each tile's base64 data URL from the renderer, then
                // register it with SandboxImages and bind the image source.
                for (let i = 0; i < gridModel.count; i++) {
                    const cell = gridModel.get(i)
                    if (cell.ty < 0 || cell.ty >= n) continue
                    requestTile(i, cell.z, cell.tx, cell.ty)
                }
            }

            function requestTile(i, z, tx, ty) {
                const r = root.core("map_render_tile", { z: z, x: tx, y: ty })
                if (r.ok && r.result && r.result.data_url) {
                    const src = SandboxImages.sourceFor(r.result.data_url)
                    gridModel.setProperty(i, "url", src)
                }
            }

            ListModel { id: gridModel }

            Repeater {
                model: gridModel
                delegate: Image {
                    x: model.wx; y: model.wy
                    width: root.tileSize; height: root.tileSize
                    source: model.url
                    fillMode: Image.Pad
                    asynchronous: true
                    Rectangle { anchors.fill: parent; color: "transparent"
                        border.color: Qt.rgba(0.961,0.961,0.941,0.04); border.width: 1; visible: false }
                }
            }

            MouseArea {
                id: nav; anchors.fill: parent; hoverEnabled: true
                property real lastX: 0; property real lastY: 0
                onPressed: (m) => { lastX = m.x; lastY = m.y }
                onPositionChanged: (m) => {
                    if (!(m.buttons & Qt.LeftButton)) return
                    const dx = m.x - lastX, dy = m.y - lastY
                    lastX = m.x; lastY = m.y
                    const z = root.zoom
                    const ctx = root.lonToX(root.centerLon, z)
                    const cty = root.latToY(root.centerLat, z)
                    root.centerLon = root.xToLon(ctx - dx / root.tileSize, z)
                    root.centerLat = root.yToLat(cty - dy / root.tileSize, z)
                    root.viewTick++
                }
                onWheel: (w) => {
                    const z0 = root.zoom
                    const dir = w.angleDelta.y > 0 ? 1 : -1
                    const z1 = Math.max(root.minZoom, Math.min(root.maxZoom, z0 + dir))
                    if (z1 === z0) return
                    const ctx = root.lonToX(root.centerLon, z0)
                    const cty = root.latToY(root.centerLat, z0)
                    const cx = ctx + (w.x - viewport.vpW/2) / root.tileSize
                    const cy = cty + (w.y - viewport.vpH/2) / root.tileSize
                    const scale = root.pow2(z1) / root.pow2(z0)
                    const ncx = cx * scale - (w.x - viewport.vpW/2) / root.tileSize
                    const ncy = cy * scale - (w.y - viewport.vpH/2) / root.tileSize
                    root.centerLon = root.xToLon(ncx, z1)
                    root.centerLat = root.yToLat(ncy, z1)
                    root.zoom = z1; root.viewTick++
                }
            }

            ColumnLayout {
                anchors.right: parent.right; anchors.bottom: parent.bottom
                anchors.margins: 12; spacing: 6
                Button { text: "+"; onClicked: { root.zoom = Math.min(root.maxZoom, root.zoom+1); root.viewTick++ } }
                Button { text: "−"; onClicked: { root.zoom = Math.max(root.minZoom, root.zoom-1); root.viewTick++ } }
            }
            Text { anchors.left: parent.left; anchors.bottom: parent.bottom; anchors.margins: 8
                text: "data distributed on Logos · renderer: osm_renderer"
                font.family: "DejaVu Sans Mono"; font.pixelSize: 10
                color: Qt.rgba(0.961,0.961,0.941,0.30) }

            Connections {
                target: root
                function onViewTickChanged() { viewport.relayout() }
                function onZoomChanged() { viewport.relayout() }
            }
            onWidthChanged: viewport.relayout()
            onHeightChanged: viewport.relayout()
            Component.onCompleted: viewport.relayout()
        }

        // Idle/preparing/error placeholder.
        Item {
            Layout.fillWidth: true; Layout.fillHeight: true
            visible: root.mapState !== "ready"
            Text {
                anchors.centerIn: parent
                text: root.mapState === "preparing"
                      ? (root.mapError || "Working…")
                      : root.mapState === "error"
                        ? root.mapError
                        : root.mapState === "loading-list"
                          ? "Loading region set…"
                          : "Pick a region and click Render map."
                font.family: "DejaVu Sans"; font.pixelSize: 15
                color: root.mapState === "error" ? "#c0654e" : Qt.rgba(0.961,0.961,0.941,0.55)
            }
        }
    }
}
