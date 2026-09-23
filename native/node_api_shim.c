#include <stdatomic.h>
#include <stddef.h>
#include <stdint.h>

typedef void* napi_env;
typedef void* napi_value;
typedef void* napi_callback_info;
typedef void* napi_handle_scope;
typedef int32_t napi_status;
typedef napi_value (*napi_callback)(napi_env env, napi_callback_info info);

typedef struct napi_vm_node_api_table {
  napi_status (*create_int32)(napi_env, int32_t, napi_value*);
  napi_status (*get_value_int32)(napi_env, napi_value, int32_t*);
  napi_status (*create_object)(napi_env, napi_value*);
  napi_status (*create_function)(napi_env, const char*, size_t, napi_callback,
                                 void*, napi_value*);
  napi_status (*set_named_property)(napi_env, napi_value, const char*, napi_value);
  napi_status (*get_named_property)(napi_env, napi_value, const char*, napi_value*);
  napi_status (*get_cb_info)(napi_env, napi_callback_info, size_t*, napi_value*,
                             napi_value*, void**);
  napi_status (*open_handle_scope)(napi_env, napi_handle_scope*);
  napi_status (*close_handle_scope)(napi_env, napi_handle_scope);
} napi_vm_node_api_table;

#if defined(_WIN32)
#define NAPI_VM_EXPORT __declspec(dllexport)
#else
#define NAPI_VM_EXPORT __attribute__((visibility("default")))
#endif

static _Atomic(const napi_vm_node_api_table*) api_table;

NAPI_VM_EXPORT void napi_vm_install_node_api_table(
    const napi_vm_node_api_table* table) {
  atomic_store_explicit(&api_table, table, memory_order_release);
}

static const napi_vm_node_api_table* get_api_table(void) {
  return atomic_load_explicit(&api_table, memory_order_acquire);
}

NAPI_VM_EXPORT napi_status napi_create_int32(napi_env env, int32_t value,
                                              napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_int32(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_int32(napi_env env, napi_value value,
                                                 int32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_int32(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_object(napi_env env, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_object(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_function(
    napi_env env, const char* name, size_t length, napi_callback callback,
    void* data, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_function(env, name, length, callback, data, result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_set_named_property(napi_env env,
                                                    napi_value object,
                                                    const char* name,
                                                    napi_value value) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->set_named_property(env, object, name, value) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_named_property(napi_env env,
                                                    napi_value object,
                                                    const char* name,
                                                    napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_named_property(env, object, name, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_cb_info(napi_env env,
                                             napi_callback_info info,
                                             size_t* argc, napi_value* argv,
                                             napi_value* this_arg, void** data) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_cb_info(env, info, argc, argv, this_arg, data) : 9;
}

NAPI_VM_EXPORT napi_status napi_open_handle_scope(napi_env env,
                                                   napi_handle_scope* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->open_handle_scope(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_close_handle_scope(napi_env env,
                                                    napi_handle_scope scope) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->close_handle_scope(env, scope) : 9;
}
