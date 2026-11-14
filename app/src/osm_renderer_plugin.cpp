#include "osm_renderer_plugin.h"

#include <QJsonDocument>
#include <QJsonObject>
#include <QTimer>

#include "logos_api.h"
#include "logos_api_client.h"

OsmRendererPlugin::OsmRendererPlugin(QObject* parent)
    : OsmRendererSimpleSource(parent)
{
}

OsmRendererPlugin::~OsmRendererPlugin()
{
    delete m_osm;
    delete m_render;
}

void OsmRendererPlugin::initLogos(LogosAPI* api)
{
    m_api = api;
    setBackend(this);
    ensureClients();
    // Surface the renderer core's version on startup (best-effort; the replica
    // may not be valid yet, in which case the empty string is fine).
    QTimer::singleShot(0, this, [this]() {});
}

void OsmRendererPlugin::ensureClients()
{
    if (m_osm && m_render) {
        return;
    }
    if (!m_api) {
        return;
    }
    // This replica is "osm_renderer"; the two cores it forwards to are "osm"
    // (the SDK) and "osm_renderer_core" (the Rust renderer).
    if (!m_osm) {
        m_osm = new LogosAPIClient(
            QStringLiteral("osm"),
            QStringLiteral("osm_renderer"),
            m_api->getTokenManager(),
            this);
    }
    if (!m_render) {
        m_render = new LogosAPIClient(
            QStringLiteral("osm_renderer_core"),
            QStringLiteral("osm_renderer"),
            m_api->getTokenManager(),
            this);
    }
}

QString OsmRendererPlugin::forward(const QString& coreName, const QString& method,
                                   const QString& name, const QString& argsJson)
{
    ensureClients();
    LogosAPIClient* client = (coreName == QLatin1String("osm")) ? m_osm : m_render;
    if (!client) {
        return QStringLiteral("{\"ok\":false,\"error\":\"no %1 client\"}").arg(coreName);
    }
    return client->invokeRemoteMethod(coreName, method, name, argsJson).toString();
}

QString OsmRendererPlugin::invokeOp(QString name, QString argsJson)
{
    // map_* ops go to the renderer core's generic invoke_op; everything else
    // (regions/lookup/fetch/catalog/open/status/...) goes to the osm core's
    // invokeOpJson — exactly the split a consumer of both modules wants.
    QString result;
    if (name.startsWith(QStringLiteral("map_"))) {
        result = forward(QStringLiteral("osm_renderer_core"),
                         QStringLiteral("invoke_op"), name, argsJson);
    } else {
        result = forward(QStringLiteral("osm"),
                         QStringLiteral("invokeOpJson"), name, argsJson);
    }
    setLastResult(result);

    const QJsonDocument doc = QJsonDocument::fromJson(result.toUtf8());
    if (doc.isObject() && !doc.object().value(QLatin1String("ok")).toBool()) {
        setLastErr(doc.object().value(QLatin1String("error")).toString());
    } else {
        setLastErr(QStringLiteral(""));
    }
    return result;
}
