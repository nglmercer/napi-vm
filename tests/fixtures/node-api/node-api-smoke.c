#include <stddef.h>
#include <stdint.h>

typedef void* napi_env;
typedef void* napi_value;
typedef int32_t napi_status;

#ifdef _WIN32
#define NAPI_VM_IMPORT __declspec(dllimport)
#define NAPI_VM_EXPORT __declspec(dllexport)
#define NAPI_VM_CALL __cdecl
#else
#define NAPI_VM_IMPORT
#define NAPI_VM_EXPORT __attribute__((visibility("default")))
#define NAPI_VM_CALL
#endif

enum { napi_ok = 0 };

NAPI_VM_IMPORT napi_status NAPI_VM_CALL napi_create_int32(napi_env env,
                                                          int32_t value,
                                                          napi_value* result);
NAPI_VM_IMPORT napi_status NAPI_VM_CALL napi_set_named_property(
    napi_env env, napi_value object, const char* name, napi_value value);

NAPI_VM_EXPORT int32_t NAPI_VM_CALL node_api_module_get_api_version_v1(void) {
  return 1;
}

NAPI_VM_EXPORT napi_value NAPI_VM_CALL napi_register_module_v1(napi_env env,
                                                                napi_value exports) {
  napi_value answer;
  if (napi_create_int32(env, 42, &answer) != napi_ok ||
      napi_set_named_property(env, exports, "answer", answer) != napi_ok) {
    return NULL;
  }
  return exports;
}
