// osm_renderer — "Map Viewer". A pannable/zoomable slippy map of an OSM region
// fetched from Logos Storage by its on-chain CID.
//
// The renderer (PBF parse -> spatial index -> raster PNG) lives in the
// osm_renderer_core module (a Rust cdylib); region resolution / on-chain
// lookup / Logos Storage fetch live in the unchanged osm module. This app's
// plugin forwards ops to both cores over RemoteObjects; QML calls
// invokeOp(name, argsJson) and reads results from the lastResult PROPERTY —
// a QML method call on a dynamic replica returns a default value immediately,
// and property sync is the channel that works (same as osm_app).
//
// Ops are serialized through one pending-op slot: results are routed by the
// "op" tag the cores embed, so at most one op is in flight and no correlation
// ambiguity exists. Tiles are requested one at a time from a queue.
//
// The Basecamp QML sandbox blocks http/file image sources, so each tile is
// handed to the host's SandboxImages capability as a base64 data URL and the
// returned image://sandbox-media/<id> URL is bound to the Image source.
import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: root
    color: "#0b1512"

    readonly property var bk: logos.module("osm_renderer")
    // The tiles dir under the app's install dir (a sandbox-allowed root, so
    // file:// URLs there load in Image.source). Resolved from the QML's own
    // location. The renderer writes z_x_y.png here and returns the file:// URL.
    readonly property string tilesDir: {
        // Qt.resolvedUrl(".") -> "file:///.../plugins/osm_renderer/qml/"
        const here = Qt.resolvedUrl(".").toString()
        const path = here.replace(/^file:\/\//, "").replace(/\/qml\/$/, "/tiles")
        return path
    }
    property string lastErr: ""
    property var regionList: []
    property string mapState: "idle"   // idle | preparing | ready | error
    property string mapError: ""
    property string fetchJobId: ""
    property var mapInfo: ({})
    property string mapRegion: ""

    // View state (the slippy map).
    property real centerLat: 0.0
    property real centerLon: 0.0
    property int  zoom: 4
    property int  minZoom: 4
    property int  maxZoom: 16
    property int  viewTick: 0
    readonly property int tileSize: 256

    // Serialized op dispatch: fire-and-route via lastResult.
    property string pendingOp: ""
    property var pendingCtx: ({})       // per-op context for routing
    property var tileQueue: []          // [{z,x,y,i}] pending tile requests

    function fire(op, args, ctx) {
        if (!root.bk) return
        root.pendingOp = op
        root.pendingCtx = ctx || ({})
        root.bk.invokeOp(op, JSON.stringify(args || {}))
    }

    function onResult(r) {
        // Route one completed op; defaults no-op so unknown results are safe.
        const op = r.op || ""
        const ctx = root.pendingCtx || ({})
        root.pendingOp = ""
        root.pendingCtx = ({})
        if (op === "open") {
            fire("regions", {})
        } else if (op === "regions") {
            if (r.ok && r.result && r.result.regions) {
                root.regionList = r.result.regions.map(g => ({ path: g.path, name: g.name }))
                const def = root.regionList.find(g => g.path === "china/henan")
                regionPicker.currentIndex = root.regionList.indexOf(def || root.regionList[0])
                root.mapState = "idle"
            } else {
                root.mapState = "error"
                root.mapError = r.error || "no regions from the osm module"
            }
        } else if (op === "lookup") {
            const region = ctx.region || ""
            if (r.ok && r.result && r.result.latest && r.result.latest.cid) {
                const latest = r.result.latest
                fire("fetch", { region: region, cid: latest.cid,
                                md5_hex: latest.checksum || "", job: true })
                root.mapError = "fetching region from Logos Storage…"
            } else {
                // No on-chain entry: fall back to the local catalog.
                fire("catalog", {}, { region: region, stage: "catalog-first" })
            }
        } else if (op === "fetch") {
            if (r.ok && r.result && r.result.job_id) {
                root.fetchJobId = r.result.job_id
                root.mapState = "preparing"
                root.mapError = "fetching region from Logos Storage…"
                fetchPoll.running = true
            } else {
                fire("catalog", {}, { region: root.mapRegion, stage: "catalog-after-fetch" })
            }
        } else if (op === "job_status") {
            if (!r.ok || !r.result) { return }
            const j = r.result
            if (j.state === "running") return
            fetchPoll.running = false
            root.fetchJobId = ""
            if (j.state === "done" && j.result && j.result.path) {
                fire("catalog", {}, { region: root.mapRegion, stage: "catalog-fetched", path: j.result.path })
            } else {
                root.mapState = "error"
                root.mapError = j.error || "fetch failed"
            }
        } else if (op === "catalog") {
            const region = ctx.region || root.mapRegion
            let pbf = ctx.path || ""
            let version = 0, md5 = "", cid = ""
            if (r.ok && r.result) {
                const rec = r.result.find(x => x.region === region)
                if (rec) { if (!pbf) pbf = rec.path || ""
                           version = rec.version || 0; md5 = rec.md5 || ""; cid = rec.cid || "" }
            }
            if (!pbf) {
                root.mapState = "error"
                root.mapError = "no local PBF for '" + region + "'. Fetch it first (osm_app), or check the storage node."
                return
            }
            root.mapState = "preparing"
            root.mapError = "building map index…"
            fire("map_prepare", { pbf: pbf, region: region, version: version, md5: md5, cid: cid })
        } else if (op === "map_prepare") {
            if (!r.ok) {
                root.mapState = "error"
                root.mapError = r.error || "map_prepare failed"
                return
            }
            statusPoll.running = true
            statusPoll.tick()
        } else if (op === "map_status") {
            if (!r.ok || !r.result) return
            const s = r.result.state
            if (s === "building") return
            statusPoll.running = false
            if (s === "ready" && r.result.info) {
                root.mapInfo = r.result.info
                const b = root.mapInfo.bounds || null
                if (b) {
                    root.centerLat = b.center_lat
                    root.centerLon = b.center_lon
                    root.zoom = Math.max(root.minZoom, Math.min(root.maxZoom,
                                Math.max(2, (root.mapInfo.fit_zoom || 6) + 1)))
                }
                root.mapState = "ready"
                root.viewTick++
            } else if (s === "error") {
                root.mapState = "error"
                root.mapError = r.result.error || "build failed"
            }
        } else if (op === "map_render_tile") {
            const t = ctx.tile
            if (r.ok && r.result && r.result.url && t) {
                // The renderer wrote the tile PNG into the app's install dir
                // (a sandbox-allowed root); Image.source takes the file:// URL.
                gridModel.setProperty(t.i, "url", r.result.url)
            } else if (t) {
                gridModel.setProperty(t.i, "url", "")
            }
            root.pumpTiles()
        }
    }

    Connections {
        target: root.bk
        function onLastResultChanged() {
            if (!root.bk || !root.bk.lastResult.length) return
            let r
            try { r = JSON.parse(root.bk.lastResult) }
            catch (e) { r = { ok: false, error: root.bk.lastResult } }
            root.onResult(r)
        }
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

    function startMap(region) {
        root.mapRegion = region
        root.mapState = "preparing"
        root.mapError = "resolving region on-chain…"
        root.fetchJobId = ""
        fire("lookup", { region: region }, { region: region })
    }

    function stopMap() {
        statusPoll.running = false; fetchPoll.running = false
        root.tileQueue = []
        fire("map_stop", {})
        root.mapState = "idle"
        root.mapInfo = ({})
    }

    // Poll a fetch job (osm core job_status; routed above). Always fire: the
    // result channel occasionally drops a frame, and gating on pendingOp would
    // then stall the poll forever. job_status is idempotent + uncorrelated.
    Timer {
        id: fetchPoll
        interval: 1000; repeat: true
        onTriggered: { if (root.fetchJobId) { root.pendingOp = ""; fire("job_status", { job_id: root.fetchJobId }) } }
    }

    // Poll the renderer build state (routed above). Same rationale: always fire.
    Timer {
        id: statusPoll
        interval: 1000; repeat: true
        function tick() { root.pendingOp = ""; fire("map_status", {}) }
        onTriggered: tick()
    }

    Component.onCompleted: { console.log("tilesDir:", root.tilesDir); fire("open", {}) }

    // Pump the tile queue: request the next tile (serialized, one in flight).
    // Called from onResult after a tile completes and from relayout to start.
    function pumpTiles() {
        if (root.tileQueue.length === 0) return
        const t = root.tileQueue.shift()
        fire("map_render_tile", { z: t.z, x: t.x, y: t.y, out_dir: root.tilesDir }, { tile: t })
    }

    // Relayout watchdog: the viewport's Layout dimensions may still be 0 when
    // the first viewTick fires (the ColumnLayout hasn't settled yet), so
    // relayout returns without queueing tiles. Retry until the grid is filled.
    Timer {
        id: relayoutTimer
        interval: 500; repeat: true
        running: root.mapState === "ready" && gridModel.count === 0
        onTriggered: viewport.relayout()
    }

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
                                       && regionPicker.currentText.length > 0
                            onClicked: root.startMap(regionPicker.currentText)
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
                    Text { visible: root.mapState === "error"
                        text: "last: " + String(root.bk ? root.bk.lastResult : "").substring(0, 140)
                        font.family: "DejaVu Sans Mono"; font.pixelSize: 9
                        color: Qt.rgba(0.961,0.961,0.941,0.40); wrapMode: Text.Wrap; Layout.maximumWidth: 360 }
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
                root.tileQueue = []
                for (let dy = 0; dy < rows; dy++) {
                    for (let dx = 0; dx < cols; dx++) {
                        const tx = Math.floor(ctx) - halfC + dx
                        const ty = Math.floor(cty) - halfR + dy
                        const wx = ox + dx * root.tileSize
                        const wy = oy + dy * root.tileSize
                        const nx = ((tx % n) + n) % n
                        const inRange = ty >= 0 && ty < n
                        const i = gridModel.count
                        gridModel.append({ wx: wx, wy: wy, url: "" })
                        if (inRange) root.tileQueue.push({ z: z, x: nx, y: ty, i: i })
                    }
                }
                root.pumpTiles()
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
                text: "data distributed on Logos · renderer: osm_renderer_core"
                font.family: "DejaVu Sans Mono"; font.pixelSize: 10
                color: Qt.rgba(0.961,0.961,0.941,0.30) }
            Text { anchors.left: parent.left; anchors.top: parent.top; anchors.margins: 8
                text: "vp " + Math.round(viewport.vpW) + "x" + Math.round(viewport.vpH)
                      + "  grid " + gridModel.count + "  queue " + root.tileQueue.length
                      + "  pend " + root.pendingOp
                      + "  lastLen " + (root.bk ? root.bk.lastResult.length : -1)
                      + "  lastHead " + String(root.bk ? root.bk.lastResult : "").substring(0, 60)
                font.family: "DejaVu Sans Mono"; font.pixelSize: 10
                color: "#ffd328" }

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
                        : root.regionList.length === 0
                          ? "Loading region set…"
                          : "Pick a region and click Render map."
                font.family: "DejaVu Sans"; font.pixelSize: 15
                color: root.mapState === "error" ? "#c0654e" : Qt.rgba(0.961,0.961,0.941,0.55)
            }
        }
    }
}
