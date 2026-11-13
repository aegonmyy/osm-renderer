#include "osm_renderer_plugin.h"

#include <QJsonDocument>
#include <QJsonObject>
#include <QTimer>

#include "logos_api.h"
#include "logos_api_client.h"

// Platform-agnostic dynamic loading of the renderer cdylib.
#ifdef _WIN32
#  include <windows.h>
#else
#  include <dlfcn.h>
#endif

namespace {
using InvokeFn   = const char* (*)(const char* name, const char* argsJson);
using VersionFn  = const char* (*)();
using FreeFn     = void (*)(const char* ptr);

// Holds the resolved C ABI of libosm_renderer.so. Loaded once, reused for the
// process lifetime. The library sits next to this plugin (RPATH $ORIGIN) under
// the name libosm_renderer.so / osm_renderer.dll, unless OSM_RENDERER_FFI_PATH
// points elsewhere.
struct RendererAbi {
    void* lib = nullptr;
    InvokeFn  invoke  = nullptr;
    VersionFn version = nullptr;
    FreeFn    free    = nullptr;
    QString   err;
};

RendererAbi& abi()
{
    static RendererAbi a;
    return a;
}

void loadAbi()
{
    RendererAbi& a = abi();
    if (a.lib) return;

    const char* envPath = std::getenv("OSM_RENDERER_FFI_PATH");
#ifdef _WIN32
    const char* kLib = "osm_renderer.dll";
    std::string p = (envPath && *envPath) ? envPath : kLib;
    a.lib = reinterpret_cast<void*>(::LoadLibraryA(p.c_str()));
    if (!a.lib) { a.err = "cannot load " + QString::fromStdString(p); return; }
    a.invoke  = reinterpret_cast<InvokeFn>(::GetProcAddress(static_cast<HMODULE>(a.lib), "osm_renderer_invoke"));
    a.version = reinterpret_cast<VersionFn>(::GetProcAddress(static_cast<HMODULE>(a.lib), "osm_renderer_version"));
    a.free    = reinterpret_cast<FreeFn>(::GetProcAddress(static_cast<HMODULE>(a.lib), "osm_renderer_free_string"));
#else
    const char* kLib = "libosm_renderer.so";
    std::string p = (envPath && *envPath) ? envPath : kLib;
    a.lib = ::dlopen(p.c_str(), RTLD_NOW | RTLD_LOCAL);
    if (!a.lib) { a.err = QString("cannot load ") + QString::fromStdString(p) + ": " + ::dlerror(); return; }
    a.invoke  = reinterpret_cast<InvokeFn>(::dlsym(a.lib, "osm_renderer_invoke"));
    a.version = reinterpret_cast<VersionFn>(::dlsym(a.lib, "osm_renderer_version"));
    a.free    = reinterpret_cast<FreeFn>(::dlsym(a.lib, "osm_renderer_free_string"));
#endif
    if (!a.invoke || !a.version || !a.free) {
        a.err = QStringLiteral("libosm_renderer missing C ABI symbols");
    }
}

QString abiInvoke(const QString& name, const QString& argsJson)
{
    loadAbi();
    RendererAbi& a = abi();
    if (!a.invoke) {
        return QStringLiteral("{\"ok\":false,\"error\":\"renderer not loaded: ")
            + a.err + QStringLiteral("\"}");
    }
    const QByteArray n = name.toUtf8();
    const QByteArray j = argsJson.toUtf8();
    const char* res = a.invoke(n.constData(), j.constData());
    QString out = QString::fromUtf8(res);
    a.free(res);
    return out;
}
} // namespace

OsmRendererPlugin::OsmRendererPlugin(QObject* parent)
    : OsmRendererSimpleSource(parent)
{
}

OsmRendererPlugin::~OsmRendererPlugin()
{
    delete m_client;
}

void OsmRendererPlugin::initLogos(LogosAPI* api)
{
    m_api = api;
    setBackend(this);
    ensureClient();
    // Surface the renderer version in the header on startup.
    loadAbi();
    RendererAbi& a = abi();
    if (a.version) {
        const char* v = a.version();
        setRendererVersion(QString::fromUtf8(v));
        a.free(v);
    } else {
        setRendererVersion(QStringLiteral("{\"ok\":false,\"error\":\"") + a.err + QStringLiteral("\"}"));
    }
    QTimer::singleShot(0, this, [this]() {
        // Nothing else to refresh at startup — the view loads the region list
        // itself via invokeOp("regions").
    });
}

void OsmRendererPlugin::ensureClient()
{
    if (m_client || !m_api) {
        return;
    }
    // The core module is named "osm"; this replica is "osm_renderer".
    m_client = new LogosAPIClient(
        QStringLiteral("osm"),
        QStringLiteral("osm_renderer"),
        m_api->getTokenManager(),
        this);
}

QString OsmRendererPlugin::invokeCore(const QString& method, const QVariantList& args)
{
    ensureClient();
    if (!m_client) {
        return QStringLiteral("{\"ok\":false,\"error\":\"no osm client\"}");
    }
    switch (args.size()) {
        case 0:
            return m_client->invokeRemoteMethod(QStringLiteral("osm"), method).toString();
        case 1:
            return m_client->invokeRemoteMethod(QStringLiteral("osm"), method, args[0]).toString();
        case 2:
            return m_client->invokeRemoteMethod(QStringLiteral("osm"), method, args[0], args[1]).toString();
        default:
            return QStringLiteral("{\"ok\":false,\"error\":\"unsupported argument count\"}");
    }
}

QString OsmRendererPlugin::invokeRenderer(const QString& name, const QString& argsJson)
{
    return abiInvoke(name, argsJson);
}

QString OsmRendererPlugin::invokeOp(QString name, QString argsJson)
{
    QString result;
    // map_* ops run against the local renderer cdylib; everything else is
    // forwarded to the osm core (regions/lookup/fetch/catalog/open/...).
    if (name.startsWith(QStringLiteral("map_"))) {
        result = invokeRenderer(name, argsJson);
    } else {
        result = invokeCore(QStringLiteral("invokeOpJson"), QVariantList{ name, argsJson });
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
