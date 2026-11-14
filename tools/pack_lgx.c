// pack_lgx.c — build/extract valid .lgx packages using the liblgx C API (the
// same library Basecamp's package manager uses).
//
//   Pack (build a .lgx from a variant dir):
//     ./pack_lgx pack out.lgx name version variant variant_dir main_file [icon]
//   Install (extract a .lgx into an installed layout):
//     ./pack_lgx install in.lgx variant dest_dir
//   Verify (an installed tree against its manifest):
//     ./pack_lgx verify dir variant
//
// The install mode replicates what Basecamp's package manager does: extract
// the variant, write manifest.json (the signed bytes from lgx_get_manifest_json)
// and the `variant` marker file beside it. This lets us test apps locally on a
// box without nix, before the nix CI build exists.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "lgx.h"

static int do_pack(int argc, char** argv) {
    const char* out       = argv[2];
    const char* name      = argv[3];
    const char* version   = argv[4];
    const char* variant   = argv[5];
    const char* files     = argv[6];
    const char* main_file = argv[7];
    const char* icon      = argc > 8 ? argv[8] : "assets/icon.png";

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

static int do_install(const char* lgx_path, const char* variant, const char* dest) {
    lgx_package_t pkg = lgx_load(lgx_path);
    if (!pkg) { fprintf(stderr, "lgx_load: %s\n", lgx_get_last_error()); return 1; }

    lgx_result_t r = lgx_extract(pkg, variant, dest);
    if (!r.success) { fprintf(stderr, "lgx_extract: %s\n", r.error ? r.error : lgx_get_last_error()); lgx_free_package(pkg); return 1; }

    // The signed manifest bytes, written beside the extracted variant — the
    // same bytes lgx_get_manifest_json() hands the installer.
    const char* manifest = lgx_get_manifest_json(pkg);
    if (!manifest) { fprintf(stderr, "no manifest\n"); lgx_free_package(pkg); return 1; }
    char mpath[4096];
    snprintf(mpath, sizeof mpath, "%s/manifest.json", dest);
    FILE* f = fopen(mpath, "wb");
    if (!f) { perror("manifest.json"); lgx_free_package(pkg); return 1; }
    fwrite(manifest, 1, strlen(manifest), f);
    fclose(f);

    // The `variant` marker naming the installed variant.
    char vpath[4096];
    snprintf(vpath, sizeof vpath, "%s/variant", dest);
    f = fopen(vpath, "wb");
    if (!f) { perror("variant"); lgx_free_package(pkg); return 1; }
    fwrite(variant, 1, strlen(variant), f);
    fputc('\n', f);
    fclose(f);

    lgx_free_package(pkg);
    printf("installed %s (%s) into %s\n", lgx_path, variant, dest);
    return 0;
}

int main(int argc, char** argv) {
    if (argc >= 8 && !strcmp(argv[1], "pack"))   return do_pack(argc, argv);
    if (argc >= 5 && !strcmp(argv[1], "install")) return do_install(argv[2], argv[3], argv[4]);
    fprintf(stderr,
        "usage:\n"
        "  %s pack out.lgx name version variant variant_dir main_file [icon]\n"
        "  %s install in.lgx variant dest_dir\n", argv[0], argv[0]);
    return 2;
}
