#ifndef OSM_RENDERER_PLUGIN_H
#define OSM_RENDERER_PLUGIN_H

#include <QString>
#include <QVariantList>

#include "rep_osm_renderer_source.h"
#include "osm_renderer_interface.h"
#include "LogosViewPluginBase.h"

class LogosAPI;
class LogosAPIClient;

// OsmRendererPlugin bridges the QML view with two core modules:
//  - `osm` (unchanged SDK) for region resolution, on-chain lookup, Logos
//    Storage fetch, and the local catalog — via its generic invokeOpJson.
//  - `osm_renderer` (the Rust renderer cdylib) for map_* ops — via its generic
//    invoke_op.
// QML calls invokeOp(name, argsJson); the plugin routes map_* to the renderer
// and everything else to osm, returning the JSON result. This is the same
// forwarding pattern OsmAppPlugin uses with the osm core.
class OsmRendererPlugin : public OsmRendererSimpleSource,
                         public OsmRendererInterface,
                         public OsmRendererViewPluginBase
{
    Q_OBJECT
    Q_PLUGIN_METADATA(IID OsmRendererInterface_iid FILE "metadata.json")
    Q_INTERFACES(OsmRendererInterface)

public:
    explicit OsmRendererPlugin(QObject* parent = nullptr);
    ~OsmRendererPlugin() override;

    QString name()    const override { return QStringLiteral("osm_renderer_app"); }
    QString version() const override { return QStringLiteral("0.1.0"); }

    Q_INVOKABLE void initLogos(LogosAPI* api);

    QString invokeOp(QString name, QString argsJson) override;

private:
    void ensureClients();
    // Forward to a core module's generic invoke entry point.
    QString forward(const QString& coreName, const QString& method,
                    const QString& name, const QString& argsJson);

    LogosAPI*       m_api    = nullptr;
    LogosAPIClient* m_osm    = nullptr;        // -> osm core (invokeOpJson)
    LogosAPIClient* m_render = nullptr;        // -> osm_renderer core (invoke_op)
};

#endif // OSM_RENDERER_PLUGIN_H
