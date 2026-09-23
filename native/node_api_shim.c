#include <stdatomic.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef void* napi_env;
typedef void* napi_value;
typedef void* napi_callback_info;
typedef void* napi_handle_scope;
typedef int32_t napi_status;
typedef int32_t napi_typedarray_type;
typedef napi_value (*napi_callback)(napi_env env, napi_callback_info info);
typedef void (*napi_finalize)(napi_env env, void* finalize_data,
                              void* finalize_hint);

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
  napi_status (*create_buffer)(napi_env, size_t, void**, napi_value*);
  napi_status (*create_buffer_copy)(napi_env, size_t, const void*, void**,
                                    napi_value*);
  napi_status (*get_buffer_info)(napi_env, napi_value, void**, size_t*);
  napi_status (*is_buffer)(napi_env, napi_value, bool*);
  napi_status (*is_arraybuffer)(napi_env, napi_value, bool*);
  napi_status (*create_arraybuffer)(napi_env, size_t, void**, napi_value*);
  napi_status (*get_arraybuffer_info)(napi_env, napi_value, void**, size_t*);
  napi_status (*is_typedarray)(napi_env, napi_value, bool*);
  napi_status (*create_typedarray)(napi_env, napi_typedarray_type, size_t,
                                   napi_value, size_t, napi_value*);
  napi_status (*get_typedarray_info)(napi_env, napi_value,
                                     napi_typedarray_type*, size_t*, void**,
                                     napi_value*, size_t*);
  napi_status (*create_dataview)(napi_env, size_t, napi_value, size_t,
                                 napi_value*);
  napi_status (*is_dataview)(napi_env, napi_value, bool*);
  napi_status (*get_dataview_info)(napi_env, napi_value, size_t*, void**,
                                   napi_value*, size_t*);
  napi_status (*create_error)(napi_env, napi_value, napi_value, napi_value*);
  napi_status (*create_type_error)(napi_env, napi_value, napi_value,
                                   napi_value*);
  napi_status (*create_range_error)(napi_env, napi_value, napi_value,
                                    napi_value*);
  napi_status (*throw_value)(napi_env, napi_value);
  napi_status (*throw_error)(napi_env, const char*, const char*);
  napi_status (*throw_type_error)(napi_env, const char*, const char*);
  napi_status (*throw_range_error)(napi_env, const char*, const char*);
  napi_status (*is_exception_pending)(napi_env, bool*);
  napi_status (*get_and_clear_last_exception)(napi_env, napi_value*);
  napi_status (*is_error)(napi_env, napi_value, bool*);
  napi_status (*create_reference)(napi_env, napi_value, uint32_t, void**);
  napi_status (*delete_reference)(napi_env, void*);
  napi_status (*reference_ref)(napi_env, void*, uint32_t*);
  napi_status (*reference_unref)(napi_env, void*, uint32_t*);
  napi_status (*get_reference_value)(napi_env, void*, napi_value*);
  napi_status (*wrap)(napi_env, napi_value, void*, napi_finalize, void*, void**);
  napi_status (*unwrap)(napi_env, napi_value, void**);
  napi_status (*remove_wrap)(napi_env, napi_value, void**);
  napi_status (*create_object)(napi_env, napi_value*);
  napi_status (*create_function)(napi_env, const char*, size_t, napi_callback,
                                 void*, napi_value*);
  napi_status (*set_named_property)(napi_env, napi_value, const char*, napi_value);
  napi_status (*get_named_property)(napi_env, napi_value, const char*, napi_value*);
  napi_status (*call_function)(napi_env, napi_value, napi_value, size_t,
                               const napi_value*, napi_value*);
  napi_status (*new_instance)(napi_env, napi_value, size_t, const napi_value*,
                              napi_value*);
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

NAPI_VM_EXPORT napi_status napi_create_buffer(napi_env env, size_t length,
                                               void** data,
                                               napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_buffer(env, length, data, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_buffer_copy(napi_env env, size_t length,
                                                    const void* data,
                                                    void** result_data,
                                                    napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_buffer_copy(env, length, data, result_data,
                                            result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_get_buffer_info(napi_env env, napi_value value,
                                                 void** data,
                                                 size_t* length) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_buffer_info(env, value, data, length) : 9;
}

NAPI_VM_EXPORT napi_status napi_is_buffer(napi_env env, napi_value value,
                                           bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_buffer(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_is_arraybuffer(napi_env env, napi_value value,
                                                bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_arraybuffer(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_arraybuffer(napi_env env,
                                                     size_t byte_length,
                                                     void** data,
                                                     napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_arraybuffer(env, byte_length, data, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_arraybuffer_info(napi_env env,
                                                      napi_value arraybuffer,
                                                      void** data,
                                                      size_t* byte_length) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_arraybuffer_info(env, arraybuffer, data,
                                              byte_length)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_is_typedarray(napi_env env, napi_value value,
                                               bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_typedarray(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_typedarray(
    napi_env env, napi_typedarray_type type, size_t length,
    napi_value arraybuffer, size_t byte_offset, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_typedarray(env, type, length, arraybuffer,
                                           byte_offset, result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_get_typedarray_info(
    napi_env env, napi_value typedarray, napi_typedarray_type* type,
    size_t* length, void** data, napi_value* arraybuffer,
    size_t* byte_offset) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_typedarray_info(env, typedarray, type, length,
                                             data, arraybuffer, byte_offset)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_create_dataview(
    napi_env env, size_t byte_length, napi_value arraybuffer,
    size_t byte_offset, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_dataview(env, byte_length, arraybuffer,
                                         byte_offset, result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_is_dataview(napi_env env, napi_value value,
                                             bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_dataview(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_dataview_info(
    napi_env env, napi_value dataview, size_t* byte_length, void** data,
    napi_value* arraybuffer, size_t* byte_offset) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_dataview_info(env, dataview, byte_length, data,
                                            arraybuffer, byte_offset)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_create_error(napi_env env, napi_value code,
                                              napi_value message,
                                              napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_error(env, code, message, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_type_error(napi_env env,
                                                   napi_value code,
                                                   napi_value message,
                                                   napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_type_error(env, code, message, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_range_error(napi_env env,
                                                    napi_value code,
                                                    napi_value message,
                                                    napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_range_error(env, code, message, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_throw(napi_env env, napi_value error) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->throw_value(env, error) : 9;
}

NAPI_VM_EXPORT napi_status napi_throw_error(napi_env env, const char* code,
                                             const char* message) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->throw_error(env, code, message) : 9;
}

NAPI_VM_EXPORT napi_status napi_throw_type_error(napi_env env,
                                                  const char* code,
                                                  const char* message) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->throw_type_error(env, code, message) : 9;
}

NAPI_VM_EXPORT napi_status napi_throw_range_error(napi_env env,
                                                   const char* code,
                                                   const char* message) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->throw_range_error(env, code, message) : 9;
}

NAPI_VM_EXPORT napi_status napi_is_exception_pending(napi_env env,
                                                      bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_exception_pending(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_and_clear_last_exception(
    napi_env env, napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_and_clear_last_exception(env, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_is_error(napi_env env, napi_value value,
                                          bool* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->is_error(env, value, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_create_reference(napi_env env,
                                                  napi_value value,
                                                  uint32_t initial_ref_count,
                                                  void** result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->create_reference(env, value, initial_ref_count, result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_delete_reference(napi_env env, void* reference) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->delete_reference(env, reference) : 9;
}

NAPI_VM_EXPORT napi_status napi_reference_ref(napi_env env, void* reference,
                                               uint32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->reference_ref(env, reference, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_reference_unref(napi_env env, void* reference,
                                                 uint32_t* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->reference_unref(env, reference, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_get_reference_value(napi_env env,
                                                     void* reference,
                                                     napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->get_reference_value(env, reference, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_wrap(napi_env env, napi_value object,
                                     void* native_object,
                                     napi_finalize finalize_cb,
                                     void* finalize_hint, void** result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->wrap(env, object, native_object, finalize_cb,
                             finalize_hint, result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_unwrap(napi_env env, napi_value object,
                                       void** result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->unwrap(env, object, result) : 9;
}

NAPI_VM_EXPORT napi_status napi_remove_wrap(napi_env env, napi_value object,
                                            void** result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->remove_wrap(env, object, result) : 9;
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

NAPI_VM_EXPORT napi_status napi_call_function(napi_env env, napi_value recv,
                                               napi_value function, size_t argc,
                                               const napi_value* argv,
                                               napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->call_function(env, recv, function, argc, argv, result)
               : 9;
}

NAPI_VM_EXPORT napi_status napi_new_instance(napi_env env,
                                              napi_value constructor,
                                              size_t argc,
                                              const napi_value* argv,
                                              napi_value* result) {
  const napi_vm_node_api_table* table = get_api_table();
  return table ? table->new_instance(env, constructor, argc, argv, result) : 9;
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
