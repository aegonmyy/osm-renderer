#ifndef OSM_RENDERER_PLUGIN_H
#define OSM_RENDERER_PLUGIN_H

#include <QString>
#include <QVariantList>

#include "rep_osm_renderer_source.h"
#include "osm_renderer_interface.h"
#include "LogosViewPluginBase.h"

class LogosAPI;
class LogosAPIClient;

// OsmRendererPlugin bridges the QML view with (a) the core `osm` module, for
// region resolution / on-chain lookup / Logos Storage fetch / the local
// catalog — forwarded over RemoteObjects via LogosAPIClient, exactly as
// OsmAppPlugin does — and (b) the local raster renderer cdylib
// (libosm_renderer.so), which it dlopens and reaches through a C ABI for the
// map_* operations (build the index from a fetched PBF, render a tile as a
// base64 PNG, status, stop). QML calls invokeOp(name, argsJson) and parses the
// returned JSON; map_* results carry a base64 data:image/png URL the view hands
// to the host's SandboxImages capability.
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

    QString name()    const override { return QStringLiteral("osm_renderer"); }
    QString version() const override { return QStringLiteral("0.1.0"); }

    Q_INVOKABLE void initLogos(LogosAPI* api);

    // Slot declared in the .rep source.
    QString invokeOp(QString name, QString argsJson) override;

private:
    void ensureClient();
    QString invokeCore(const QString& method, const QVariantList& args = {});
    // Local renderer dispatch (map_* ops). Returns JSON.
    QString invokeRenderer(const QString& name, const QString& argsJson);

    LogosAPI*       m_api    = nullptr;
    LogosAPIClient* m_client = nullptr;
};

#endif // OSM_RENDERER_PLUGIN_H
