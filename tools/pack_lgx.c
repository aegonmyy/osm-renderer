// pack_lgx.c — build a valid .lgx package from a variant directory using the
// liblgx C API (the same library Basecamp's package manager uses). This lets
// us produce an installable, integrity-hashed .lgx on a box without nix, so
// the app can be tested locally before the nix CI build exists.
//
//   cc -o pack_lgx pack_lgx.c -L/tmp/squashfs-root/usr/lib -llgx -lsodium
//        -Wl,-rpath,/tmp/squashfs-root/usr/lib
//   ./pack_lgx out.lgx osm_renderer 0.1.0 linux-amd64 variant_dir osm_renderer_plugin.so
//
// The variant_dir must contain every file the variant ships: the plugin .so,
// libosm_renderer.so, qml/Main.qml, qml/qmldir, metadata.json, assets/icon.png,
// and any bundled libs. liblgx computes the variant + assets hashes; we print
// the resulting manifest so it can be written beside an installed tree.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lgx.h"

int main(int argc, char** argv) {
    if (argc < 7) {
        fprintf(stderr, "usage: %s out.lgx name version variant variant_dir main_file [icon_rel] [description]\n", argv[0]);
        return 2;
    }
    const char* out       = argv[1];
    const char* name      = argv[2];
    const char* version   = argv[3];
    const char* variant   = argv[4];
    const char* files     = argv[5];
    const char* main_file = argv[6];
    const char* icon      = argc > 7 ? argv[7] : "assets/icon.png";

    lgx_result_t r;
    r = lgx_create(out, name);
    if (!r.success) { fprintf(stderr, "lgx_create: %s\n", r.error ? r.error : lgx_get_last_error()); return 1; }

    lgx_package_t pkg = lgx_load(out);
    if (!pkg) { fprintf(stderr, "lgx_load: %s\n", lgx_get_last_error()); return 1; }

    r = lgx_set_version(pkg, version);
    if (!r.success) { fprintf(stderr, "lgx_set_version: %s\n", r.error ? r.error : lgx_get_last_error()); lgx_free_package(pkg); return 1; }
    lgx_set_icon(pkg, icon);

    r = lgx_add_variant(pkg, variant, files, main_file);
    if (!r.success) { fprintf(stderr, "lgx_add_variant: %s\n", r.error ? r.error : lgx_get_last_error()); lgx_free_package(pkg); return 1; }

    r = lgx_save(pkg, out);
    if (!r.success) { fprintf(stderr, "lgx_save: %s\n", r.error ? r.error : lgx_get_last_error()); lgx_free_package(pkg); return 1; }

    const char* manifest = lgx_get_manifest_json(pkg);
    printf("%s\n", manifest ? manifest : "{}");

    lgx_free_package(pkg);
    printf("packed %s\n", out);
    return 0;
}
