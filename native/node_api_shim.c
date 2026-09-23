#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef void* napi_env;
typedef void* napi_value;
typedef void* napi_callback_info;
typedef void* napi_handle_scope;
typedef int32_t napi_status;
typedef napi_value (*napi_callback)(napi_env env, napi_callback_info info);

typedef struct napi_vm_node_api_table {
  napi_status (*get_undefined)(napi_env, napi_value*);
  napi_status (*get_null)(napi_env, napi_value*);
  napi_status (*get_boolean)(napi_env, bool, napi_value*);
  napi_status (*create_double)(napi_env, double, napi_value*);
  napi_status (*create_int32)(napi_env, int32_t, napi_value*);
  napi_status (*create_uint32)(napi_env, uint32_t, napi_value*);
  napi_status (*create_int64)(napi_env, int64_t, napi_value*);
  napi_status (*create_string_utf8)(napi_env, const char*, size_t, napi_value*);
  napi_status (*typeof_value)(napi_env, napi_value, int32_t*);
  napi_status (*get_value_double)(napi_env, napi_value, double*);
  napi_status (*get_value_int32)(napi_env, napi_value, int32_t*);
  napi_status (*get_value_uint32)(napi_env, napi_value, uint32_t*);
  napi_status (*get_value_int64)(napi_env, napi_value, int64_t*);
  napi_status (*get_value_bool)(napi_env, napi_value, bool*);
  napi_status (*get_value_string_utf8)(napi_env, napi_value, char*, size_t,
                                       size_t*);
  napi_status (*create_array)(napi_env, napi_value*);
  napi_status (*create_array_with_length)(napi_env, size_t, napi_value*);
  napi_status (*is_array)(napi_env, napi_value, bool*);
  napi_status (*get_array_length)(napi_env, napi_value, uint32_t*);
  napi_status (*get_element)(napi_env, napi_value, uint32_t, napi_value*);
  napi_status (*set_element)(napi_env, napi_value, uint32_t, napi_value);
  napi_status (*has_element)(napi_env, napi_value, uint32_t, bool*);
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

NAPI_VM_EXPORT napi_status napi_get_undefined(napi_env env, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_undefined(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_null(napi_env env, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_null(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_boolean(napi_env env, bool value,
                                            napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_boolean(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_double(napi_env env, double value,
                                               napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_double(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_int32(napi_env env, int32_t value,
                                              napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_int32(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_uint32(napi_env env, uint32_t value,
                                               napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_uint32(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_int64(napi_env env, int64_t value,
                                              napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_int64(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_string_utf8(napi_env env,
                                                    const char* value,
                                                    size_t length,
                                                    napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_string_utf8(env, value, length, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_typeof(napi_env env, napi_value value,
                                        int32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->typeof_value(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_double(napi_env env,
                                                  napi_value value,
                                                  double* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_double(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_int32(napi_env env, napi_value value,
                                                 int32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_int32(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_uint32(napi_env env,
                                                  napi_value value,
                                                  uint32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_uint32(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_int64(napi_env env,
                                                 napi_value value,
                                                 int64_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_int64(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_bool(napi_env env, napi_value value,
                                                bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_bool(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_value_string_utf8(napi_env env,
                                                        napi_value value,
                                                        char* buffer,
                                                        size_t buffer_size,
                                                        size_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_value_string_utf8(env, value, buffer, buffer_size,
                                               result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_create_array(napi_env env,
                                              napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_array(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_array_with_length(napi_env env,
                                                          size_t length,
                                                          napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_array_with_length(env, length, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_is_array(napi_env env, napi_value value,
                                          bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_array(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_array_length(napi_env env,
                                                  napi_value value,
                                                  uint32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_array_length(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_element(napi_env env, napi_value value,
                                             uint32_t index,
                                             napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_element(env, value, index, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_set_element(napi_env env, napi_value value,
                                             uint32_t index,
                                             napi_value element) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->set_element(env, value, index, element) : 9;
}

NAPI_VM_EXPORT napi_status napi_has_element(napi_env env, napi_value value,
                                             uint32_t index, bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->has_element(env, value, index, result) : 9;
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
