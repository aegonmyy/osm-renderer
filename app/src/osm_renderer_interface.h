#ifndef OSM_RENDERER_INTERFACE_H
#define OSM_RENDERER_INTERFACE_H

#include <QObject>
#include <QString>

#include "interface.h"

// Marker interface for the osm_renderer ("Map Viewer") view plugin. All logic
// lives in OsmRendererPlugin; this header supplies the IID used by
// Q_PLUGIN_METADATA and Q_DECLARE_INTERFACE.
class OsmRendererInterface : public PluginInterface
{
public:
    virtual ~OsmRendererInterface() = default;
};

#define OsmRendererInterface_iid "org.logos.OsmRendererInterface/1"
Q_DECLARE_INTERFACE(OsmRendererInterface, OsmRendererInterface_iid)

#endif // OSM_RENDERER_INTERFACE_H
