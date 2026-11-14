/* Minimal lgx.h — only the C API the local packer uses, from logos-package/src/lgx.h. */
#ifndef LGX_H
#define LGX_H
#include <stddef.h>
#include <stdbool.h>
#ifndef LGX_EXPORT
#if defined(_WIN32) || defined(__CYGWIN__)
  #define LGX_EXPORT __declspec(dllimport)
#else
  #define LGX_EXPORT __attribute__((visibility("default")))
#endif
#endif
#ifdef __cplusplus
extern "C" {
#endif
typedef struct lgx_package_opaque* lgx_package_t;
typedef struct { bool success; const char* error; } lgx_result_t;
LGX_EXPORT lgx_result_t lgx_create(const char* output_path, const char* name);
LGX_EXPORT lgx_package_t lgx_load(const char* path);
LGX_EXPORT lgx_result_t lgx_save(lgx_package_t pkg, const char* path);
LGX_EXPORT lgx_result_t lgx_add_variant(lgx_package_t pkg, const char* variant,
                                        const char* files_path, const char* main_path);
LGX_EXPORT lgx_result_t lgx_extract(lgx_package_t pkg, const char* variant, const char* output_dir);
LGX_EXPORT lgx_result_t lgx_set_version(lgx_package_t pkg, const char* version);
LGX_EXPORT void lgx_set_description(lgx_package_t pkg, const char* description);
LGX_EXPORT void lgx_set_icon(lgx_package_t pkg, const char* icon);
LGX_EXPORT const char* lgx_get_manifest_json(lgx_package_t pkg);
LGX_EXPORT void lgx_free_package(lgx_package_t pkg);
LGX_EXPORT const char* lgx_get_last_error(void);

/* Installed-tree integrity (from the full logos-package lgx.h). */
typedef enum {
    LGX_INTEGRITY_OK = 0,
    LGX_INTEGRITY_MISMATCH = 1,
    LGX_INTEGRITY_NO_HASH = 2,
    LGX_INTEGRITY_UNREADABLE = 3,
    LGX_INTEGRITY_BAD_INPUT = 4
} lgx_integrity_t;
LGX_EXPORT lgx_integrity_t lgx_verify_installed_tree(
    const char* dir_path, const char* manifest_bytes, size_t manifest_len, const char* variant);
#ifdef __cplusplus
}
#endif
#endif
