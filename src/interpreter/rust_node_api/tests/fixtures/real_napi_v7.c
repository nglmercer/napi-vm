
#define _POSIX_C_SOURCE 200809L
#define NAPI_VERSION 7
#include <node_api.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

static napi_ref persistent_values;
static napi_ref removable_object;
static napi_ref wrapped_object_reference;
static napi_ref external_value_reference;
static int wrapped_finalizer_calls;
static int added_finalizer_calls;
static int removed_finalizer_calls;
static int external_finalizer_calls;
static int external_arraybuffer_finalizer_calls;
static int external_buffer_finalizer_calls;
static uint8_t* external_arraybuffer_data;
static uint8_t detachable_arraybuffer_data[8] = {9, 8, 7, 6, 5, 4, 3, 2};
static uint8_t* external_buffer_data;
static int finalizer_create_function_status = -1;
static int cleanup_hook_order[4];
static int cleanup_hook_count;
static int cleanup_before_wrap_finalizer;
static napi_env cleanup_env;
static int descriptor_setter_value = 5;
static int class_constructor_offset = 1;
static int counter_static_offset = 8;
static int target_call_count;
static int target_construct_count;
static bool counter_new_target_seen;
static bool counter_child_new_target_seen;
static int threadsafe_finalizer_calls;
static int threadsafe_worker_context_ok;
static int threadsafe_worker_call_status = -1;
static int threadsafe_worker_blocking_status = -1;
static int threadsafe_queue_first_status = -1;
static int threadsafe_queue_full_status = -1;
static int threadsafe_abort_call_status = -1;
static atomic_int threadsafe_abort_ready;
static char threadsafe_context_marker;
static char threadsafe_finalize_marker;
static napi_deferred pending_promise_deferred;
static char* copy_text(const char* text) {
  size_t length = strlen(text) + 1;
  char* copy = (char*)malloc(length);
  if (copy != NULL) memcpy(copy, text, length);
  return copy;
}
typedef struct async_work_context {
  napi_deferred deferred;
  napi_async_work work;
  int32_t result;
} async_work_context;
static char wrapped_native_data[] = "wrapped-native-data";
static char removable_native_data[] = "removed-native-data";
static char external_native_data[] = "external-native-data";
static char external_finalize_hint;
static char added_finalizer_data;
static char added_finalizer_hint;
static char instance_data_first_marker;
static char instance_data_second_marker;
static char instance_data_finalize_hint;
static int instance_data_finalizer_calls;
static int replaced_instance_data_finalizer_calls;
static int instance_data_visible_in_finalizer;

static void finalize_external_arraybuffer(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  if (data == external_arraybuffer_data) {
    external_arraybuffer_finalizer_calls++;
    free(data);
    external_arraybuffer_data = NULL;
  }
}

static void finalize_external_buffer(napi_env env, void* data, void* hint) {
  (void)env;
  (void)hint;
  if (data == external_buffer_data) {
    external_buffer_finalizer_calls++;
    free(data);
    external_buffer_data = NULL;
  }
}

static napi_value finalizer_noop(napi_env env, napi_callback_info info) {
  (void)env;
  (void)info;
  return NULL;
}

static void finalize_probe(napi_env env, void* data, void* hint) {
  (void)hint;
  if (data == wrapped_native_data) {
    napi_value ignored;
    wrapped_finalizer_calls++;
    cleanup_before_wrap_finalizer = cleanup_hook_count == 2;
    finalizer_create_function_status = napi_create_function(
        env, "fromFinalizer", NAPI_AUTO_LENGTH, finalizer_noop, NULL, &ignored);
  }
  if (data == removable_native_data) removed_finalizer_calls++;
}

static void finalize_added_date(napi_env env, void* data, void* hint) {
  (void)env;
  if (data == &added_finalizer_data && hint == &added_finalizer_hint)
    added_finalizer_calls++;
}

static void finalize_instance_data(napi_env env, void* data, void* hint) {
  if (data == &instance_data_second_marker &&
      hint == &instance_data_finalize_hint) {
    void* current_data = NULL;
    instance_data_finalizer_calls++;
    if (napi_get_instance_data(env, &current_data) == napi_ok &&
        current_data == data)
      instance_data_visible_in_finalizer++;
  }
  if (data == &instance_data_first_marker)
    replaced_instance_data_finalizer_calls++;
}

static void finalize_external_probe(napi_env env, void* data, void* hint) {
  (void)env;
  if (data == external_native_data && hint == &external_finalize_hint)
    external_finalizer_calls++;
}

static void cleanup_probe(void* arg) {
  if (cleanup_hook_count < 4) {
    cleanup_hook_order[cleanup_hook_count++] = (int)(intptr_t)arg;
  }
}

static void cleanup_remove_other(void* arg) {
  if (napi_remove_env_cleanup_hook(cleanup_env, cleanup_probe, arg) != napi_ok) {
    cleanup_probe((void*)(intptr_t)-1);
    return;
  }
  cleanup_probe((void*)(intptr_t)4);
}

static napi_value cleanup_misuse_status(napi_env env, napi_callback_info info) {
  napi_value result, status_value;
  napi_status duplicate_status, unmatched_status;
  (void)info;
  if (napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)5) != napi_ok)
    return NULL;
  duplicate_status = napi_add_env_cleanup_hook(env, cleanup_probe,
                                               (void*)(intptr_t)5);
  unmatched_status = napi_remove_env_cleanup_hook(env, cleanup_probe,
                                                  (void*)(intptr_t)6);
  if (napi_remove_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)5) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, duplicate_status, &status_value) != napi_ok ||
      napi_set_named_property(env, result, "duplicate", status_value) != napi_ok ||
      napi_create_int32(env, unmatched_status, &status_value) != napi_ok ||
      napi_set_named_property(env, result, "unmatched", status_value) != napi_ok)
    return NULL;
  return result;
}

static napi_value error_info_probe(napi_env env, napi_callback_info info) {
  napi_value input, result, field;
  const napi_extended_error_info* error_info = NULL;
  double number = 0;
  napi_status last_status;
  bool message_matches;
  (void)info;
  if (napi_create_string_utf8(env, "not a number", NAPI_AUTO_LENGTH,
                              &input) != napi_ok)
    return NULL;
  last_status = napi_get_value_double(env, input, &number);
  if (last_status != napi_number_expected ||
      napi_get_last_error_info(env, &error_info) != napi_ok ||
      error_info == NULL)
    return NULL;
  message_matches = strcmp(error_info->error_message, "number expected") == 0;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, last_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "lastStatus", field) != napi_ok ||
      napi_get_boolean(env, message_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "messageMatches", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value date_probe(napi_env env, napi_callback_info info) {
  napi_value date, result, field, number, reference_value, global, date_constructor;
  napi_value date_prototype, observed_prototype;
  napi_ref weak_reference;
  double date_value = 0;
  bool is_date = false, number_is_date = true, reference_matches = false;
  bool napi_instance = false, prototype_matches = false;
  napi_status invalid_date_status;
  (void)info;
  if (napi_create_date(env, 1700000000123.0, &date) != napi_ok ||
      napi_is_date(env, date, &is_date) != napi_ok || !is_date ||
      napi_get_date_value(env, date, &date_value) != napi_ok ||
      napi_add_finalizer(env, date, &added_finalizer_data, finalize_added_date,
                         &added_finalizer_hint, &weak_reference) != napi_ok ||
      napi_get_reference_value(env, weak_reference, &reference_value) != napi_ok ||
      napi_strict_equals(env, date, reference_value, &reference_matches) != napi_ok ||
      !reference_matches ||
      napi_create_int32(env, 7, &number) != napi_ok ||
      napi_is_date(env, number, &number_is_date) != napi_ok || number_is_date ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Date", &date_constructor) != napi_ok ||
      napi_get_named_property(env, date_constructor, "prototype", &date_prototype) != napi_ok ||
      napi_get_prototype(env, date, &observed_prototype) != napi_ok ||
      napi_strict_equals(env, observed_prototype, date_prototype, &prototype_matches) != napi_ok ||
      !prototype_matches ||
      napi_instanceof(env, date, date_constructor, &napi_instance) != napi_ok ||
      !napi_instance)
    return NULL;
  invalid_date_status = napi_get_date_value(env, number, &date_value);
  if (invalid_date_status != napi_date_expected ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "date", date) != napi_ok ||
      napi_create_double(env, 1700000000123.0, &field) != napi_ok ||
      napi_set_named_property(env, result, "value", field) != napi_ok ||
      napi_get_boolean(env, is_date, &field) != napi_ok ||
      napi_set_named_property(env, result, "isDate", field) != napi_ok ||
      napi_get_boolean(env, reference_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "referenceMatches", field) != napi_ok ||
      napi_get_boolean(env, napi_instance, &field) != napi_ok ||
      napi_set_named_property(env, result, "napiInstance", field) != napi_ok ||
      napi_get_boolean(env, number_is_date, &field) != napi_ok ||
      napi_set_named_property(env, result, "numberIsDate", field) != napi_ok ||
      napi_get_boolean(env, prototype_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "prototypeMatches", field) != napi_ok ||
      napi_create_int32(env, invalid_date_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "invalidDateStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value bigint_probe(napi_env env, napi_callback_info info) {
  napi_value signed_value, unsigned_value, wide_value, result, field;
  napi_value signed_roundtrip, unsigned_roundtrip, wrapped_signed, wrapped_unsigned;
  int64_t signed_out = 0, wrapped_signed_out = 0;
  uint64_t unsigned_out = 0, wrapped_unsigned_out = 0;
  bool signed_lossless = false, unsigned_lossless = false;
  bool wrapped_signed_lossless = true, wrapped_unsigned_lossless = true;
  bool invalid_lossless = false;
  uint64_t wide_words[] = {UINT64_C(0x0123456789abcdef), UINT64_C(1)};
  uint64_t read_words[2] = {0, 0};
  int sign_bit = 0;
  size_t word_count = 0;
  napi_status invalid_type_status;
  (void)info;
  if (napi_create_bigint_int64(env, INT64_MIN, &signed_value) != napi_ok ||
      napi_create_bigint_uint64(env, UINT64_MAX, &unsigned_value) != napi_ok ||
      napi_create_bigint_words(env, 1, 2, wide_words, &wide_value) != napi_ok ||
      napi_get_value_bigint_int64(env, signed_value, &signed_out,
                                  &signed_lossless) != napi_ok ||
      napi_get_value_bigint_uint64(env, unsigned_value, &unsigned_out,
                                   &unsigned_lossless) != napi_ok ||
      napi_get_value_bigint_int64(env, unsigned_value, &wrapped_signed_out,
                                  &wrapped_signed_lossless) != napi_ok ||
      napi_get_value_bigint_uint64(env, signed_value, &wrapped_unsigned_out,
                                   &wrapped_unsigned_lossless) != napi_ok ||
      napi_get_value_bigint_words(env, wide_value, NULL, &word_count, NULL) != napi_ok ||
      word_count != 2)
    return NULL;
  word_count = 2;
  if (napi_get_value_bigint_words(env, wide_value, &sign_bit, &word_count,
                                  read_words) != napi_ok ||
      word_count != 2 || sign_bit != 1 || read_words[0] != wide_words[0] ||
      read_words[1] != wide_words[1] ||
      napi_create_bigint_int64(env, signed_out, &signed_roundtrip) != napi_ok ||
      napi_create_bigint_uint64(env, unsigned_out, &unsigned_roundtrip) != napi_ok ||
      napi_create_bigint_int64(env, wrapped_signed_out, &wrapped_signed) != napi_ok ||
      napi_create_bigint_uint64(env, wrapped_unsigned_out, &wrapped_unsigned) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "signed", signed_value) != napi_ok ||
      napi_set_named_property(env, result, "unsigned", unsigned_value) != napi_ok ||
      napi_set_named_property(env, result, "wide", wide_value) != napi_ok ||
      napi_set_named_property(env, result, "signedRoundtrip", signed_roundtrip) != napi_ok ||
      napi_set_named_property(env, result, "unsignedRoundtrip", unsigned_roundtrip) != napi_ok ||
      napi_set_named_property(env, result, "wrappedSigned", wrapped_signed) != napi_ok ||
      napi_set_named_property(env, result, "wrappedUnsigned", wrapped_unsigned) != napi_ok ||
      napi_get_boolean(env, signed_lossless, &field) != napi_ok ||
      napi_set_named_property(env, result, "signedLossless", field) != napi_ok ||
      napi_get_boolean(env, unsigned_lossless, &field) != napi_ok ||
      napi_set_named_property(env, result, "unsignedLossless", field) != napi_ok ||
      napi_get_boolean(env, wrapped_signed_lossless, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrappedSignedLossless", field) != napi_ok ||
      napi_get_boolean(env, wrapped_unsigned_lossless, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrappedUnsignedLossless", field) != napi_ok ||
      napi_create_int32(env, sign_bit, &field) != napi_ok ||
      napi_set_named_property(env, result, "signBit", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)word_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "wordCount", field) != napi_ok)
    return NULL;
  invalid_type_status = napi_get_value_bigint_int64(env, field, &signed_out,
                                                     &invalid_lossless);
  if (napi_create_int32(env, invalid_type_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "invalidTypeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value instance_data_probe(napi_env env, napi_callback_info info) {
  void* data = NULL;
  napi_value result;
  (void)info;
  if (napi_get_instance_data(env, &data) != napi_ok ||
      napi_get_boolean(env, data == &instance_data_second_marker, &result) != napi_ok)
    return NULL;
  return result;
}

static bool property_name_array_has(napi_env env, napi_value names, const char* expected_name) {
  uint32_t length = 0;
  napi_value expected;
  if (napi_get_array_length(env, names, &length) != napi_ok ||
      napi_create_string_utf8(env, expected_name, NAPI_AUTO_LENGTH, &expected) != napi_ok)
    return false;
  for (uint32_t index = 0; index < length; index++) {
    napi_value name;
    bool equal = false;
    if (napi_get_element(env, names, index, &name) != napi_ok ||
        napi_strict_equals(env, name, expected, &equal) != napi_ok)
      return false;
    if (equal) return true;
  }
  return false;
}

static napi_value proxy_property_names_probe(napi_env env, napi_callback_info info) {
  napi_value args[1], result, all_own, enumerable, skip_strings, with_prototype;
  napi_value writable, configurable;
  size_t argc = 1;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_all_property_names(env, args[0], napi_key_own_only,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &all_own) != napi_ok ||
      napi_get_all_property_names(env, args[0], napi_key_own_only,
                                  napi_key_enumerable,
                                  napi_key_numbers_to_strings, &enumerable) != napi_ok ||
      napi_get_all_property_names(env, args[0], napi_key_own_only,
                                  napi_key_skip_strings,
                                  napi_key_numbers_to_strings, &skip_strings) != napi_ok ||
      napi_get_all_property_names(env, args[0], napi_key_include_prototypes,
                                  napi_key_enumerable,
                                  napi_key_numbers_to_strings, &with_prototype) != napi_ok ||
      napi_get_all_property_names(env, args[0], napi_key_own_only,
                                  napi_key_writable,
                                  napi_key_numbers_to_strings, &writable) != napi_ok ||
      napi_get_all_property_names(env, args[0], napi_key_own_only,
                                  napi_key_configurable,
                                  napi_key_numbers_to_strings, &configurable) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "allOwn", all_own) != napi_ok ||
      napi_set_named_property(env, result, "enumerable", enumerable) != napi_ok ||
      napi_set_named_property(env, result, "skipStrings", skip_strings) != napi_ok ||
      napi_set_named_property(env, result, "withPrototype", with_prototype) != napi_ok ||
      napi_set_named_property(env, result, "writable", writable) != napi_ok ||
      napi_set_named_property(env, result, "configurable", configurable) != napi_ok)
    return NULL;
  return result;
}

static napi_value global_property_names_probe(napi_env env, napi_callback_info info) {
  napi_value global, result, own_names, all_names, field;
  bool has_object, has_global_this, prototype_supported, has_object_prototype_names = true;
  const char* object_prototype_names[] = {
      "constructor", "__defineGetter__", "__defineSetter__", "hasOwnProperty",
      "__lookupGetter__", "__lookupSetter__", "isPrototypeOf",
      "propertyIsEnumerable", "toLocaleString", "toString", "valueOf", "__proto__"};
  (void)info;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_get_all_property_names(env, global, napi_key_own_only,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &own_names) != napi_ok)
    return NULL;
  has_object = property_name_array_has(env, own_names, "Object");
  has_global_this = property_name_array_has(env, own_names, "globalThis");
  prototype_supported = napi_get_all_property_names(
      env, global, napi_key_include_prototypes, napi_key_all_properties,
      napi_key_numbers_to_strings, &all_names) == napi_ok;
  if (prototype_supported) {
    for (size_t index = 0;
         index < sizeof(object_prototype_names) / sizeof(object_prototype_names[0]);
         index++) {
      has_object_prototype_names = has_object_prototype_names &&
          property_name_array_has(env, all_names, object_prototype_names[index]);
    }
  } else {
    has_object_prototype_names = false;
  }
  if (napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, has_object, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasObject", field) != napi_ok ||
      napi_get_boolean(env, has_global_this, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasGlobalThis", field) != napi_ok ||
      napi_get_boolean(env, prototype_supported, &field) != napi_ok ||
      napi_set_named_property(env, result, "prototypeSupported", field) != napi_ok ||
      napi_get_boolean(env, has_object_prototype_names, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasObjectPrototypeNames", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value property_names_probe(napi_env env, napi_callback_info info) {
  napi_value args[4], target, class_target, function_target, promise_target, result, all_own, enumerable, skip_strings;
  napi_value with_prototype, keep_numbers, writable, configurable, class_names;
  napi_value probe_array, array_element, array_names, function_names, function_property;
  napi_value promise_names, promise_has_then, promise_has_catch, promise_has_finally;
  napi_property_descriptor function_descriptor = {
      .utf8name = "definedByNapi",
      .attributes = napi_writable | napi_configurable,
  };
  size_t argc = 4;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 4)
    return NULL;
  target = args[0];
  class_target = args[1];
  function_target = args[2];
  promise_target = args[3];
  if (napi_create_int32(env, 23, &function_property) != napi_ok ||
      napi_set_named_property(env, function_target, "nativeProperty", function_property) != napi_ok ||
      napi_create_int32(env, 42, &function_property) != napi_ok)
    return NULL;
  function_descriptor.value = function_property;
  if (napi_define_properties(env, function_target, 1, &function_descriptor) != napi_ok)
    return NULL;
  if (
      napi_get_all_property_names(env, target, napi_key_own_only,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &all_own) != napi_ok ||
      napi_get_all_property_names(env, target, napi_key_own_only,
                                  napi_key_enumerable,
                                  napi_key_numbers_to_strings, &enumerable) != napi_ok ||
      napi_get_all_property_names(env, target, napi_key_own_only,
                                  napi_key_skip_strings,
                                  napi_key_numbers_to_strings, &skip_strings) != napi_ok ||
      napi_get_all_property_names(env, target, napi_key_include_prototypes,
                                  napi_key_enumerable,
                                  napi_key_numbers_to_strings, &with_prototype) != napi_ok ||
      napi_get_all_property_names(env, target, napi_key_own_only,
                                  napi_key_enumerable,
                                  napi_key_keep_numbers, &keep_numbers) != napi_ok ||
      napi_get_all_property_names(env, target, napi_key_own_only,
                                  napi_key_writable,
                                  napi_key_numbers_to_strings, &writable) != napi_ok ||
      napi_get_all_property_names(env, target, napi_key_own_only,
                                  napi_key_configurable,
                                  napi_key_numbers_to_strings, &configurable) != napi_ok ||
      napi_get_all_property_names(env, class_target, napi_key_own_only,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &class_names) != napi_ok ||
      napi_get_all_property_names(env, function_target, napi_key_own_only,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &function_names) != napi_ok ||
      napi_get_all_property_names(env, promise_target, napi_key_include_prototypes,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &promise_names) != napi_ok ||
      napi_create_array_with_length(env, 2, &probe_array) != napi_ok ||
      napi_create_int32(env, 7, &array_element) != napi_ok ||
      napi_set_element(env, probe_array, 0, array_element) != napi_ok ||
      napi_get_all_property_names(env, probe_array, napi_key_own_only,
                                  napi_key_all_properties,
                                  napi_key_numbers_to_strings, &array_names) != napi_ok ||
      napi_get_boolean(env, property_name_array_has(env, promise_names, "then"),
                       &promise_has_then) != napi_ok ||
      napi_get_boolean(env, property_name_array_has(env, promise_names, "catch"),
                       &promise_has_catch) != napi_ok ||
      napi_get_boolean(env, property_name_array_has(env, promise_names, "finally"),
                       &promise_has_finally) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "allOwn", all_own) != napi_ok ||
      napi_set_named_property(env, result, "enumerable", enumerable) != napi_ok ||
      napi_set_named_property(env, result, "skipStrings", skip_strings) != napi_ok ||
      napi_set_named_property(env, result, "withPrototype", with_prototype) != napi_ok ||
      napi_set_named_property(env, result, "keepNumbers", keep_numbers) != napi_ok ||
      napi_set_named_property(env, result, "writable", writable) != napi_ok ||
      napi_set_named_property(env, result, "configurable", configurable) != napi_ok ||
      napi_set_named_property(env, result, "classNames", class_names) != napi_ok ||
      napi_set_named_property(env, result, "functionNames", function_names) != napi_ok ||
      napi_set_named_property(env, result, "arrayNames", array_names) != napi_ok ||
      napi_set_named_property(env, result, "promiseHasThen", promise_has_then) != napi_ok ||
      napi_set_named_property(env, result, "promiseHasCatch", promise_has_catch) != napi_ok ||
      napi_set_named_property(env, result, "promiseHasFinally", promise_has_finally) != napi_ok)
    return NULL;
  return result;
}

int napi_vm_test_cleanup_hook_count(void) {
  return cleanup_hook_count;
}

int napi_vm_test_cleanup_hook_value(int index) {
  return index >= 0 && index < cleanup_hook_count ? cleanup_hook_order[index] : -1;
}

int napi_vm_test_cleanup_before_wrap_finalizer(void) {
  return cleanup_before_wrap_finalizer;
}

int napi_vm_test_wrapped_finalizer_calls(void) {
  return wrapped_finalizer_calls;
}

int napi_vm_test_added_finalizer_calls(void) {
  return added_finalizer_calls;
}

int napi_vm_test_instance_data_finalizer_calls(void) {
  return instance_data_finalizer_calls;
}

int napi_vm_test_replaced_instance_data_finalizer_calls(void) {
  return replaced_instance_data_finalizer_calls;
}

int napi_vm_test_instance_data_visible_in_finalizer(void) {
  return instance_data_visible_in_finalizer;
}

int napi_vm_test_removed_finalizer_calls(void) {
  return removed_finalizer_calls;
}

int napi_vm_test_external_finalizer_calls(void) {
  return external_finalizer_calls;
}

int napi_vm_test_external_arraybuffer_finalizer_calls(void) {
  return external_arraybuffer_finalizer_calls;
}

int napi_vm_test_external_buffer_finalizer_calls(void) {
  return external_buffer_finalizer_calls;
}

static napi_value make_external_arraybuffer(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  external_arraybuffer_data = (uint8_t*)malloc(4);
  if (external_arraybuffer_data == NULL) return NULL;
  external_arraybuffer_data[0] = 11;
  external_arraybuffer_data[1] = 22;
  external_arraybuffer_data[2] = 33;
  external_arraybuffer_data[3] = 44;
  if (napi_create_external_arraybuffer(env, external_arraybuffer_data, 4,
                                       finalize_external_arraybuffer, NULL,
                                       &result) != napi_ok) {
    free(external_arraybuffer_data);
    external_arraybuffer_data = NULL;
    return NULL;
  }
  return result;
}

static napi_value check_external_arraybuffer(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  void* data = NULL;
  bool matches;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_arraybuffer_info(env, argv[0], &data, &length) != napi_ok)
    return NULL;
  matches = data == external_arraybuffer_data && length == 4 &&
      ((uint8_t*)data)[0] == 11 && ((uint8_t*)data)[1] == 22 &&
      ((uint8_t*)data)[2] == 77 && ((uint8_t*)data)[3] == 44;
  if (napi_get_boolean(env, matches, &result) != napi_ok) return NULL;
  return result;
}

static napi_value arraybuffer_detachment_probe(napi_env env, napi_callback_info info) {
  napi_value result, owned, external, view, data_view, field;
  napi_status owned_detach_status, detach_status, second_detach_status;
  napi_status non_arraybuffer_status;
  bool detached_before = true, detached_after = false;
  bool detached_non_arraybuffer = false;
  size_t arraybuffer_length = 99, view_length = 99, byte_offset = 99;
  size_t data_view_length = 99, data_view_offset = 99;
  napi_typedarray_type view_type = napi_uint8_array;
  void* arraybuffer_data = detachable_arraybuffer_data;
  void* view_data = detachable_arraybuffer_data;
  void* data_view_data = detachable_arraybuffer_data;
  (void)info;
  if (napi_create_arraybuffer(env, 4, NULL, &owned) != napi_ok)
    return NULL;
  owned_detach_status = napi_detach_arraybuffer(env, owned);
  if (napi_create_external_arraybuffer(env, detachable_arraybuffer_data,
                                       sizeof(detachable_arraybuffer_data),
                                       NULL, NULL, &external) != napi_ok ||
      napi_create_typedarray(env, napi_uint8_array, 4, external, 2, &view) != napi_ok ||
      napi_create_dataview(env, 4, external, 1, &data_view) != napi_ok ||
      napi_is_detached_arraybuffer(env, external, &detached_before) != napi_ok)
    return NULL;
  detach_status = napi_detach_arraybuffer(env, external);
  if (napi_is_detached_arraybuffer(env, external, &detached_after) != napi_ok ||
      napi_get_arraybuffer_info(env, external, &arraybuffer_data,
                                &arraybuffer_length) != napi_ok ||
      napi_get_typedarray_info(env, view, &view_type, &view_length, &view_data,
                               NULL, &byte_offset) != napi_ok ||
      napi_get_dataview_info(env, data_view, &data_view_length, &data_view_data,
                             NULL, &data_view_offset) != napi_ok)
    return NULL;
  second_detach_status = napi_detach_arraybuffer(env, external);
  non_arraybuffer_status = napi_is_detached_arraybuffer(env, view,
                                                         &detached_non_arraybuffer);
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, owned_detach_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "ownedDetachStatus", field) != napi_ok ||
      napi_create_int32(env, detach_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "detachStatus", field) != napi_ok ||
      napi_create_int32(env, second_detach_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "secondDetachStatus", field) != napi_ok ||
      napi_create_int32(env, non_arraybuffer_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "nonArrayBufferStatus", field) != napi_ok ||
      napi_create_int32(env, (int32_t)arraybuffer_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "arraybufferLength", field) != napi_ok ||
      napi_create_int32(env, (int32_t)view_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "viewLength", field) != napi_ok ||
      napi_create_int32(env, (int32_t)byte_offset, &field) != napi_ok ||
      napi_set_named_property(env, result, "byteOffset", field) != napi_ok ||
      napi_create_int32(env, (int32_t)data_view_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "dataViewLength", field) != napi_ok ||
      napi_create_int32(env, (int32_t)data_view_offset, &field) != napi_ok ||
      napi_set_named_property(env, result, "dataViewOffset", field) != napi_ok ||
      napi_get_boolean(env, detached_before, &field) != napi_ok ||
      napi_set_named_property(env, result, "detachedBefore", field) != napi_ok ||
      napi_get_boolean(env, detached_after, &field) != napi_ok ||
      napi_set_named_property(env, result, "detachedAfter", field) != napi_ok ||
      napi_get_boolean(env, detached_non_arraybuffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "detachedNonArrayBuffer", field) != napi_ok ||
      napi_set_named_property(env, result, "buffer", external) != napi_ok ||
      napi_set_named_property(env, result, "view", view) != napi_ok ||
      napi_set_named_property(env, result, "dataView", data_view) != napi_ok)
    return NULL;
  return result;
}

static napi_value make_external_buffer(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  external_buffer_data = (uint8_t*)malloc(4);
  if (external_buffer_data == NULL) return NULL;
  external_buffer_data[0] = 5;
  external_buffer_data[1] = 6;
  external_buffer_data[2] = 7;
  external_buffer_data[3] = 8;
  if (napi_create_external_buffer(env, 4, external_buffer_data,
                                 finalize_external_buffer, NULL,
                                 &result) != napi_ok) {
    free(external_buffer_data);
    external_buffer_data = NULL;
    return NULL;
  }
  return result;
}

static napi_value check_external_buffer(napi_env env, napi_callback_info info) {
  size_t argc = 1, length = 0;
  napi_value argv[1], result;
  void* data = NULL;
  bool is_buffer = false, matches;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_is_buffer(env, argv[0], &is_buffer) != napi_ok || !is_buffer ||
      napi_get_buffer_info(env, argv[0], &data, &length) != napi_ok)
    return NULL;
  matches = data == external_buffer_data && length == 4 &&
      ((uint8_t*)data)[0] == 5 && ((uint8_t*)data)[1] == 88 &&
      ((uint8_t*)data)[2] == 7 && ((uint8_t*)data)[3] == 8;
  if (napi_get_boolean(env, matches, &result) != napi_ok) return NULL;
  return result;
}

static napi_value async_context_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], resource, resource_name, label, argument, callback_result;
  napi_value result, field, global, microtask_flag;
  napi_async_context context;
  napi_callback_scope scope, nested_scope;
  napi_status nested_close_status, close_status, destroy_status;
  bool microtask_ran_before_return = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_create_object(env, &resource) != napi_ok ||
      napi_create_string_utf8(env, "native-resource", NAPI_AUTO_LENGTH, &label) != napi_ok ||
      napi_set_named_property(env, resource, "label", label) != napi_ok ||
      napi_create_string_utf8(env, "napi-vm/async-context-probe",
                              NAPI_AUTO_LENGTH, &resource_name) != napi_ok ||
      napi_async_init(env, resource, resource_name, &context) != napi_ok ||
      napi_open_callback_scope(env, resource, context, &scope) != napi_ok ||
      napi_open_callback_scope(env, resource, context, &nested_scope) != napi_ok ||
      (nested_close_status = napi_close_callback_scope(env, nested_scope)) != napi_ok ||
      (close_status = napi_close_callback_scope(env, scope)) != napi_ok ||
      napi_create_string_utf8(env, "callback-value", NAPI_AUTO_LENGTH, &argument) != napi_ok ||
      napi_make_callback(env, context, resource, argv[0], 1, &argument,
                         &callback_result) != napi_ok ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "asyncContextMicrotaskRan", &microtask_flag) != napi_ok ||
      napi_get_value_bool(env, microtask_flag, &microtask_ran_before_return) != napi_ok)
    return NULL;
  destroy_status = napi_async_destroy(env, context);
  if (napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "callbackResult", callback_result) != napi_ok ||
      napi_create_int32(env, nested_close_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "nestedCloseStatus", field) != napi_ok ||
      napi_create_int32(env, close_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "closeStatus", field) != napi_ok ||
      napi_create_int32(env, destroy_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "destroyStatus", field) != napi_ok ||
      napi_get_boolean(env, microtask_ran_before_return, &field) != napi_ok ||
      napi_set_named_property(env, result, "microtaskRanBeforeReturn", field) != napi_ok)
    return NULL;
  return result;
}

int napi_vm_test_finalizer_create_function_status(void) {
  return finalizer_create_function_status;
}

static napi_value add(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  int32_t left = 0, right = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_int32(env, argv[0], &left) != napi_ok ||
      napi_get_value_int32(env, argv[1], &right) != napi_ok ||
      napi_create_int32(env, left + right, &result) != napi_ok) return NULL;
  return result;
}

static napi_value round_trip(napi_env env, napi_callback_info info) {
  size_t argc = 5, length = 0, copied = 0;
  napi_value argv[5], result, field;
  napi_valuetype bool_type, number_type, string_type;
  bool flag = false;
  double number = 0;
  uint32_t uint32_value = 0;
  int64_t int64_value = 0;
  char text[128];
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 5 ||
      napi_get_value_bool(env, argv[0], &flag) != napi_ok ||
      napi_get_value_double(env, argv[1], &number) != napi_ok ||
      napi_get_value_string_utf8(env, argv[2], NULL, 0, &length) != napi_ok ||
      length >= sizeof(text) ||
      napi_get_value_string_utf8(env, argv[2], text, sizeof(text), &copied) != napi_ok ||
      copied != length ||
      napi_get_value_uint32(env, argv[3], &uint32_value) != napi_ok ||
      napi_get_value_int64(env, argv[4], &int64_value) != napi_ok ||
      napi_typeof(env, argv[0], &bool_type) != napi_ok ||
      napi_typeof(env, argv[1], &number_type) != napi_ok ||
      napi_typeof(env, argv[2], &string_type) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, flag, &field) != napi_ok ||
      napi_set_named_property(env, result, "flag", field) != napi_ok ||
      napi_create_double(env, number + 0.5, &field) != napi_ok ||
      napi_set_named_property(env, result, "number", field) != napi_ok ||
      napi_create_string_utf8(env, text, copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "text", field) != napi_ok ||
      napi_create_uint32(env, uint32_value, &field) != napi_ok ||
      napi_set_named_property(env, result, "uint32", field) != napi_ok ||
      napi_create_int64(env, int64_value, &field) != napi_ok ||
      napi_set_named_property(env, result, "int64", field) != napi_ok ||
      napi_create_int32(env, bool_type, &field) != napi_ok ||
      napi_set_named_property(env, result, "boolType", field) != napi_ok ||
      napi_create_int32(env, number_type, &field) != napi_ok ||
      napi_set_named_property(env, result, "numberType", field) != napi_ok ||
      napi_create_int32(env, string_type, &field) != napi_ok ||
      napi_set_named_property(env, result, "stringType", field) != napi_ok) return NULL;
  return result;
}

static napi_value int64_conversion_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  int64_t value = 0;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_int64(env, argv[0], &value) != napi_ok ||
      napi_create_double(env, (double)value, &result) != napi_ok) return NULL;
  return result;
}

static napi_value string_encoding_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1, required = 0, copied = 0, truncated_copied = 0;
  size_t wrong_type_length = 0;
  napi_value argv[1], result, created, bytes, field, number, truncated_text;
  napi_status wrong_type_status;
  const char latin1[] = {'A', '\0', (char)0xE9, (char)0xFF};
  char buffer[16] = {0};
  char truncated[4] = {'x', 'x', 'x', 'x'};
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_string_latin1(env, argv[0], NULL, 0, &required) != napi_ok ||
      required >= sizeof(buffer) ||
      napi_get_value_string_latin1(env, argv[0], buffer, sizeof(buffer), &copied) != napi_ok ||
      napi_get_value_string_latin1(env, argv[0], truncated, sizeof(truncated),
                                   &truncated_copied) != napi_ok ||
      napi_create_string_latin1(env, latin1, sizeof(latin1), &created) != napi_ok ||
      napi_create_string_latin1(env, truncated, truncated_copied, &truncated_text) != napi_ok ||
      napi_create_array_with_length(env, copied, &bytes) != napi_ok ||
      napi_create_int32(env, 1, &number) != napi_ok)
    return NULL;
  for (size_t index = 0; index < copied; index++) {
    if (napi_create_uint32(env, (unsigned char)buffer[index], &field) != napi_ok ||
        napi_set_element(env, bytes, (uint32_t)index, field) != napi_ok)
      return NULL;
  }
  wrong_type_status = napi_get_value_string_latin1(
      env, number, NULL, 0, &wrong_type_length);
  if (wrong_type_status != napi_string_expected ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "created", created) != napi_ok ||
      napi_set_named_property(env, result, "bytes", bytes) != napi_ok ||
      napi_create_uint32(env, (uint32_t)required, &field) != napi_ok ||
      napi_set_named_property(env, result, "required", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "copied", field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedText", truncated_text) != napi_ok ||
      napi_create_uint32(env, (uint32_t)truncated_copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedCopied", field) != napi_ok ||
      napi_create_int32(env, wrong_type_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrongTypeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value utf16_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1, required = 0, copied = 0, truncated_copied = 0;
  size_t wrong_type_length = 0;
  napi_value argv[1], result, round_trip, units, truncated_units, field, number;
  napi_value auto_length_value;
  napi_status wrong_type_status;
  const char16_t nul_terminated[] = {'T', 'E', 'R', 'M', '\0', 'X', '\0'};
  char16_t buffer[16] = {0};
  char16_t truncated[4] = {0};
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_value_string_utf16(env, argv[0], NULL, 0, &required) != napi_ok ||
      required >= sizeof(buffer) / sizeof(buffer[0]) ||
      napi_get_value_string_utf16(env, argv[0], buffer,
                                  sizeof(buffer) / sizeof(buffer[0]), &copied) != napi_ok ||
      copied != required ||
      napi_get_value_string_utf16(env, argv[0], truncated,
                                  sizeof(truncated) / sizeof(truncated[0]),
                                  &truncated_copied) != napi_ok ||
      napi_create_string_utf16(env, buffer, copied, &round_trip) != napi_ok ||
      napi_create_string_utf16(env, nul_terminated, NAPI_AUTO_LENGTH,
                               &auto_length_value) != napi_ok ||
      napi_create_array_with_length(env, copied, &units) != napi_ok ||
      napi_create_array_with_length(env, truncated_copied, &truncated_units) != napi_ok ||
      napi_create_int32(env, 1, &number) != napi_ok)
    return NULL;
  for (size_t index = 0; index < copied; index++) {
    if (napi_create_uint32(env, buffer[index], &field) != napi_ok ||
        napi_set_element(env, units, (uint32_t)index, field) != napi_ok)
      return NULL;
  }
  for (size_t index = 0; index < truncated_copied; index++) {
    if (napi_create_uint32(env, truncated[index], &field) != napi_ok ||
        napi_set_element(env, truncated_units, (uint32_t)index, field) != napi_ok)
      return NULL;
  }
  wrong_type_status = napi_get_value_string_utf16(
      env, number, NULL, 0, &wrong_type_length);
  if (wrong_type_status != napi_string_expected ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "roundTrip", round_trip) != napi_ok ||
      napi_set_named_property(env, result, "autoLength", auto_length_value) != napi_ok ||
      napi_set_named_property(env, result, "units", units) != napi_ok ||
      napi_create_uint32(env, (uint32_t)required, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "copied", field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedUnits", truncated_units) != napi_ok ||
      napi_create_uint32(env, (uint32_t)truncated_copied, &field) != napi_ok ||
      napi_set_named_property(env, result, "truncatedCopied", field) != napi_ok ||
      napi_create_int32(env, wrong_type_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "wrongTypeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value invalid_utf16_status(napi_env env, napi_callback_info info) {
  const char16_t invalid[] = {0xD800};
  const napi_extended_error_info* error_info = NULL;
  napi_value ignored, result, field;
  napi_status status = napi_create_string_utf16(env, invalid, 1, &ignored);
  bool message_matches;
  (void)info;
  if (status != napi_generic_failure ||
      napi_get_last_error_info(env, &error_info) != napi_ok || error_info == NULL)
    return NULL;
  message_matches = strcmp(error_info->error_message,
      "UTF-16 input is malformed or exceeds napi-vm string limits") == 0;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, status, &field) != napi_ok ||
      napi_set_named_property(env, result, "status", field) != napi_ok ||
      napi_get_boolean(env, message_matches, &field) != napi_ok ||
      napi_set_named_property(env, result, "messageMatches", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_bool_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_bool(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_number_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_number(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_string_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_string(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value coerce_to_object_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value argv[1], result;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 1 ||
      napi_coerce_to_object(env, argv[0], &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value delete_element_probe(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result, field;
  uint32_t index = 0, length = 0;
  bool deleted = false, present = true;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_get_value_uint32(env, argv[1], &index) != napi_ok ||
      napi_delete_element(env, argv[0], index, &deleted) != napi_ok ||
      napi_delete_element(env, argv[0], index, NULL) != napi_ok ||
      napi_has_element(env, argv[0], index, &present) != napi_ok ||
      napi_get_array_length(env, argv[0], &length) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, deleted, &field) != napi_ok ||
      napi_set_named_property(env, result, "deleted", field) != napi_ok ||
      napi_get_boolean(env, present, &field) != napi_ok ||
      napi_set_named_property(env, result, "present", field) != napi_ok ||
      napi_create_uint32(env, length, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value escapable_scope_probe(napi_env env, napi_callback_info info) {
  napi_escapable_handle_scope scope;
  napi_value local, escaped, ignored, field, result, escaped_value;
  napi_status second_escape_status;
  (void)info;
  if (napi_open_escapable_handle_scope(env, &scope) != napi_ok ||
      napi_create_object(env, &local) != napi_ok ||
      napi_create_int32(env, 42, &field) != napi_ok ||
      napi_set_named_property(env, local, "value", field) != napi_ok ||
      napi_escape_handle(env, scope, local, &escaped) != napi_ok)
    return NULL;
  second_escape_status = napi_escape_handle(env, scope, local, &ignored);
  if (second_escape_status != napi_escape_called_twice ||
      napi_close_escapable_handle_scope(env, scope) != napi_ok ||
      napi_get_named_property(env, escaped, "value", &escaped_value) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "escaped", escaped_value) != napi_ok ||
      napi_create_int32(env, (int32_t)second_escape_status, &field) != napi_ok ||
      napi_set_named_property(env, result, "secondEscapeStatus", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value run_script_probe(napi_env env, napi_callback_info info) {
  const char source[] =
      "globalThis.napiRunScriptCount = (globalThis.napiRunScriptCount || 0) + 1; "
      "globalThis.napiRunScriptMicrotask = false; "
      "Promise.resolve().then(() => { globalThis.napiRunScriptMicrotask = true; }); "
      "6 * 7";
  napi_value script, value, global, microtask_value, result, field;
  bool microtask_ran_during_call = true;
  (void)info;
  if (napi_create_string_utf8(env, source, sizeof(source) - 1, &script) != napi_ok ||
      napi_run_script(env, script, &value) != napi_ok ||
      napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "napiRunScriptMicrotask",
                              &microtask_value) != napi_ok ||
      napi_get_value_bool(env, microtask_value, &microtask_ran_during_call) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "value", value) != napi_ok ||
      napi_get_boolean(env, microtask_ran_during_call, &field) != napi_ok ||
      napi_set_named_property(env, result, "microtaskRanDuringCall", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value invalid_environment(napi_env env, napi_callback_info info) {
  napi_value ignored, result;
  napi_status status = napi_get_null((napi_env)(uintptr_t)1, &ignored);
  (void)info;
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value buffer_probe(napi_env env, napi_callback_info info) {
  const uint8_t seed[] = {65, 66, 67, 68};
  napi_value copied, allocated, array, result, field;
  void* copied_data = NULL;
  void* allocated_data = NULL;
  size_t copied_length = 0, allocated_length = 0;
  bool copied_is_buffer = false, allocated_is_buffer = false, array_is_buffer = true;
  (void)info;
  if (napi_create_buffer_copy(env, sizeof(seed), seed, &copied_data, &copied) != napi_ok ||
      copied_data == NULL ||
      napi_get_buffer_info(env, copied, &copied_data, &copied_length) != napi_ok ||
      copied_length != sizeof(seed) ||
      napi_create_buffer(env, 3, &allocated_data, &allocated) != napi_ok ||
      allocated_data == NULL ||
      napi_get_buffer_info(env, allocated, &allocated_data, &allocated_length) != napi_ok ||
      allocated_length != 3 ||
      napi_create_array(env, &array) != napi_ok ||
      napi_is_buffer(env, copied, &copied_is_buffer) != napi_ok ||
      napi_is_buffer(env, allocated, &allocated_is_buffer) != napi_ok ||
      napi_is_buffer(env, array, &array_is_buffer) != napi_ok) return NULL;
  ((uint8_t*)copied_data)[1] = 120;
  ((uint8_t*)allocated_data)[0] = 7;
  ((uint8_t*)allocated_data)[1] = 8;
  ((uint8_t*)allocated_data)[2] = 9;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "copy", copied) != napi_ok ||
      napi_set_named_property(env, result, "allocated", allocated) != napi_ok ||
      napi_get_boolean(env, copied_is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "copyIsBuffer", field) != napi_ok ||
      napi_get_boolean(env, allocated_is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "allocatedIsBuffer", field) != napi_ok ||
      napi_get_boolean(env, array_is_buffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "arrayIsBuffer", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)copied_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "copyLength", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)allocated_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "allocatedLength", field) != napi_ok) return NULL;
  return result;
}

static napi_value invalid_typedarray(napi_env env, napi_callback_info info) {
  napi_value buffer, invalid;
  napi_value result;
  void* bytes = NULL;
  (void)info;
  if (napi_create_arraybuffer(env, 8, &bytes, &buffer) != napi_ok || bytes == NULL) return NULL;
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 3, &invalid) != napi_ok) return NULL;
  if (napi_get_undefined(env, &result) != napi_ok) return NULL;
  return result;
}

static napi_value call_guest(napi_env env, napi_callback_info info) {
  napi_value args[3], result;
  size_t argc = 3;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 3) return NULL;
  if (napi_call_function(env, args[1], args[0], 1, &args[2], &result) != napi_ok) return NULL;
  return result;
}

static napi_value construct_guest(napi_env env, napi_callback_info info) {
  napi_value args[2], result;
  size_t argc = 2;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 2) return NULL;
  if (napi_new_instance(env, args[0], 1, &args[1], &result) != napi_ok) return NULL;
  return result;
}

static napi_value resolved_promise(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, resolution;
  bool is_promise = false;
  (void)info;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_is_promise(env, promise, &is_promise) != napi_ok || !is_promise ||
      napi_create_string_utf8(env, "resolved-from-addon", NAPI_AUTO_LENGTH,
                              &resolution) != napi_ok ||
      napi_resolve_deferred(env, deferred, resolution) != napi_ok) return NULL;
  return promise;
}

static void async_work_execute(napi_env env, void* data) {
  async_work_context* context = (async_work_context*)data;
  (void)env;
  context->result = 42;
}

static void async_work_complete(napi_env env, napi_status status, void* data) {
  async_work_context* context = (async_work_context*)data;
  napi_value result;
  if (status == napi_ok &&
      napi_create_int32(env, context->result, &result) == napi_ok) {
    (void)napi_resolve_deferred(env, context->deferred, result);
  } else {
    (void)napi_create_int32(env, status, &result);
    (void)napi_reject_deferred(env, context->deferred, result);
  }
  (void)napi_delete_async_work(env, context->work);
  free(context);
}

static napi_value run_async_work(napi_env env, napi_callback_info info) {
  async_work_context* context = (async_work_context*)calloc(1, sizeof(*context));
  napi_value promise, resource_name;
  (void)info;
  if (context == NULL ||
      napi_create_promise(env, &context->deferred, &promise) != napi_ok ||
      napi_create_string_utf8(env, "napi-vm-test-async-work", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_create_async_work(env, NULL, resource_name, async_work_execute,
                             async_work_complete, context, &context->work) != napi_ok) {
    free(context);
    return NULL;
  }
  if (napi_queue_async_work(env, context->work) != napi_ok) {
    (void)napi_delete_async_work(env, context->work);
    free(context);
    return NULL;
  }
  return promise;
}

typedef struct threadsafe_work {
  napi_threadsafe_function function;
} threadsafe_work;

static void threadsafe_finalize(napi_env env, void* data, void* hint) {
  (void)env;
  if (data == &threadsafe_finalize_marker && hint == &threadsafe_context_marker) {
    threadsafe_finalizer_calls++;
  }
}

static void threadsafe_call_js(napi_env env, napi_value callback,
                               void* context, void* data) {
  if (env != NULL && callback != NULL &&
      context == &threadsafe_context_marker && data != NULL) {
    napi_value receiver, argument, ignored;
    if (napi_get_undefined(env, &receiver) == napi_ok &&
        napi_create_string_utf8(env, (const char*)data, NAPI_AUTO_LENGTH,
                                &argument) == napi_ok) {
      (void)napi_call_function(env, receiver, callback, 1, &argument, &ignored);
    }
  }
  free(data);
}

static void* threadsafe_worker(void* data) {
  threadsafe_work* work = (threadsafe_work*)data;
  void* context = NULL;
  threadsafe_worker_context_ok =
      napi_get_threadsafe_function_context(work->function, &context) == napi_ok &&
      context == &threadsafe_context_marker;
  char* message = copy_text("threadsafe-value");
  threadsafe_worker_call_status = message != NULL
      ? napi_call_threadsafe_function(work->function, message,
                                      napi_tsfn_nonblocking)
      : napi_generic_failure;
  if (threadsafe_worker_call_status != napi_ok) free(message);
  char* second = copy_text("threadsafe-second");
  threadsafe_worker_blocking_status = second != NULL
      ? napi_call_threadsafe_function(work->function, second,
                                      napi_tsfn_blocking)
      : napi_generic_failure;
  if (threadsafe_worker_blocking_status != napi_ok) free(second);
  (void)napi_release_threadsafe_function(work->function, napi_tsfn_release);
  free(work);
  return NULL;
}

static void* threadsafe_aborted_worker(void* data) {
  threadsafe_work* work = (threadsafe_work*)data;
  while (!atomic_load_explicit(&threadsafe_abort_ready, memory_order_acquire)) { }
  threadsafe_abort_call_status =
      napi_call_threadsafe_function(work->function, NULL, napi_tsfn_nonblocking);
  (void)napi_release_threadsafe_function(work->function, napi_tsfn_release);
  return NULL;
}

static napi_value run_threadsafe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value callback, resource_name, result;
  napi_threadsafe_function function;
  threadsafe_work* work = (threadsafe_work*)calloc(1, sizeof(threadsafe_work));
  pthread_t thread;
  if (work == NULL ||
      napi_get_cb_info(env, info, &argc, &callback, NULL, NULL) != napi_ok ||
      argc != 1 ||
      napi_create_string_utf8(env, "napi-vm-threadsafe-worker", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_get_undefined(env, &result) != napi_ok ||
      napi_create_threadsafe_function(env, callback, NULL, resource_name, 1, 1,
                                      &threadsafe_finalize_marker,
                                      threadsafe_finalize,
                                      &threadsafe_context_marker,
                                      threadsafe_call_js,
                                      &work->function) != napi_ok ||
      napi_unref_threadsafe_function(env, work->function) != napi_ok ||
      napi_ref_threadsafe_function(env, work->function) != napi_ok ||
                                      napi_acquire_threadsafe_function(work->function) != napi_ok) {
    free(work);
    return NULL;
  }
  function = work->function;
  if (pthread_create(&thread, NULL, threadsafe_worker, work) != 0) {
    (void)napi_release_threadsafe_function(work->function, napi_tsfn_abort);
    (void)napi_release_threadsafe_function(work->function, napi_tsfn_release);
    free(work);
    return NULL;
  }
  (void)pthread_detach(thread);
  (void)napi_release_threadsafe_function(function, napi_tsfn_release);
  return result;
}

static napi_value probe_threadsafe_queue(napi_env env,
                                         napi_callback_info info) {
  size_t argc = 1;
  napi_value callback, resource_name, result;
  napi_threadsafe_function function;
  char* first = copy_text("queue-first");
  char* second = copy_text("queue-second");
  if (napi_get_cb_info(env, info, &argc, &callback, NULL, NULL) != napi_ok ||
      argc != 1 || first == NULL || second == NULL ||
      napi_create_string_utf8(env, "napi-vm-threadsafe-queue", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_create_threadsafe_function(env, callback, NULL, resource_name, 1, 1,
                                      NULL, NULL, &threadsafe_context_marker,
                                      threadsafe_call_js, &function) != napi_ok) {
    free(first);
    free(second);
    return NULL;
  }
  threadsafe_queue_first_status =
      napi_call_threadsafe_function(function, first, napi_tsfn_nonblocking);
  if (threadsafe_queue_first_status != napi_ok) free(first);
  threadsafe_queue_full_status =
      napi_call_threadsafe_function(function, second, napi_tsfn_nonblocking);
  if (threadsafe_queue_full_status != napi_ok) free(second);
  if (napi_release_threadsafe_function(function, napi_tsfn_release) != napi_ok ||
      napi_create_int32(env, threadsafe_queue_full_status, &result) != napi_ok) {
    return NULL;
  }
  return result;
}

static napi_value probe_threadsafe_abort(napi_env env,
                                         napi_callback_info info) {
  napi_value resource_name, result;
  threadsafe_work work = {0};
  pthread_t thread;
  (void)info;
  if (napi_create_string_utf8(env, "napi-vm-threadsafe-abort", NAPI_AUTO_LENGTH,
                              &resource_name) != napi_ok ||
      napi_create_threadsafe_function(env, NULL, NULL, resource_name, 1, 2,
                                      &threadsafe_finalize_marker,
                                      threadsafe_finalize,
                                      &threadsafe_context_marker,
                                      threadsafe_call_js, &work.function) != napi_ok) {
    return NULL;
  }
  atomic_store_explicit(&threadsafe_abort_ready, 0, memory_order_release);
  if (pthread_create(&thread, NULL, threadsafe_aborted_worker, &work) != 0) {
    (void)napi_release_threadsafe_function(work.function, napi_tsfn_abort);
    return NULL;
  }
  if (napi_release_threadsafe_function(work.function, napi_tsfn_abort) != napi_ok) {
    atomic_store_explicit(&threadsafe_abort_ready, 1, memory_order_release);
    (void)pthread_join(thread, NULL);
    return NULL;
  }
  atomic_store_explicit(&threadsafe_abort_ready, 1, memory_order_release);
  (void)pthread_join(thread, NULL);
  if (napi_create_int32(env, threadsafe_abort_call_status, &result) != napi_ok) {
    return NULL;
  }
  return result;
}

int napi_vm_test_threadsafe_finalizer_calls(void) {
  return threadsafe_finalizer_calls;
}

int napi_vm_test_threadsafe_worker_context_ok(void) {
  return threadsafe_worker_context_ok;
}

int napi_vm_test_threadsafe_worker_call_status(void) {
  return threadsafe_worker_call_status;
}

int napi_vm_test_threadsafe_worker_blocking_status(void) {
  return threadsafe_worker_blocking_status;
}

static napi_value rejected_promise(napi_env env, napi_callback_info info) {
  napi_deferred deferred;
  napi_value promise, rejection;
  bool is_promise = false;
  (void)info;
  if (napi_create_promise(env, &deferred, &promise) != napi_ok ||
      napi_is_promise(env, promise, &is_promise) != napi_ok || !is_promise ||
      napi_create_string_utf8(env, "rejected-from-addon", NAPI_AUTO_LENGTH,
                              &rejection) != napi_ok ||
      napi_reject_deferred(env, deferred, rejection) != napi_ok) return NULL;
  return promise;
}

static napi_value pending_promise(napi_env env, napi_callback_info info) {
  napi_value promise;
  bool is_promise = false;
  (void)info;
  if (pending_promise_deferred != NULL ||
      napi_create_promise(env, &pending_promise_deferred, &promise) != napi_ok ||
      napi_is_promise(env, promise, &is_promise) != napi_ok || !is_promise) return NULL;
  return promise;
}

static napi_value resolve_pending_promise(napi_env env, napi_callback_info info) {
  napi_value resolution, result;
  size_t argc = 1;
  if (pending_promise_deferred == NULL ||
      napi_get_cb_info(env, info, &argc, &resolution, NULL, NULL) != napi_ok || argc < 1 ||
      napi_resolve_deferred(env, pending_promise_deferred, resolution) != napi_ok ||
      napi_get_boolean(env, true, &result) != napi_ok) return NULL;
  pending_promise_deferred = NULL;
  return result;
}

static napi_value property_probe(napi_env env, napi_callback_info info) {
  napi_value args[1], object, computed_key, inherited_key, assigned_key;
  napi_value remove_key, computed, assigned, names, result, field;
  size_t argc = 1;
  bool has_inherited = false, has_named_inherited = false;
  bool has_own_inherited = true, deleted = false;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 1) return NULL;
  object = args[0];
  if (napi_create_string_utf8(env, "computed", NAPI_AUTO_LENGTH, &computed_key) != napi_ok ||
      napi_create_string_utf8(env, "inherited", NAPI_AUTO_LENGTH, &inherited_key) != napi_ok ||
      napi_create_string_utf8(env, "assigned", NAPI_AUTO_LENGTH, &assigned_key) != napi_ok ||
      napi_create_string_utf8(env, "removeMe", NAPI_AUTO_LENGTH, &remove_key) != napi_ok ||
      napi_get_property(env, object, computed_key, &computed) != napi_ok ||
      napi_has_property(env, object, inherited_key, &has_inherited) != napi_ok ||
      napi_has_named_property(env, object, "inherited", &has_named_inherited) != napi_ok ||
      napi_has_own_property(env, object, inherited_key, &has_own_inherited) != napi_ok ||
      napi_create_string_utf8(env, "set through addon", NAPI_AUTO_LENGTH, &assigned) != napi_ok ||
      napi_set_property(env, object, assigned_key, assigned) != napi_ok ||
      napi_delete_property(env, object, remove_key, &deleted) != napi_ok ||
      napi_get_property_names(env, object, &names) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "computed", computed) != napi_ok ||
      napi_get_boolean(env, has_inherited, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasInherited", field) != napi_ok ||
      napi_get_boolean(env, has_named_inherited, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasNamedInherited", field) != napi_ok ||
      napi_get_boolean(env, has_own_inherited, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasOwnInherited", field) != napi_ok ||
      napi_get_boolean(env, deleted, &field) != napi_ok ||
      napi_set_named_property(env, result, "deleted", field) != napi_ok ||
      napi_set_named_property(env, result, "names", names) != napi_ok) return NULL;
  return result;
}

static napi_value global_probe(napi_env env, napi_callback_info info) {
  napi_value global, object_constructor, result;
  napi_valuetype type;
  (void)info;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_get_named_property(env, global, "Object", &object_constructor) != napi_ok ||
      napi_typeof(env, object_constructor, &type) != napi_ok ||
      napi_get_boolean(env, type == napi_function, &result) != napi_ok) return NULL;
  return result;
}

static napi_value defined_method(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, 42, &result) != napi_ok) return NULL;
  return result;
}

static napi_value defined_getter(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, descriptor_setter_value, &result) != napi_ok) return NULL;
  return result;
}

static napi_value defined_setter(napi_env env, napi_callback_info info) {
  napi_value value;
  size_t argc = 1;
  if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok || argc < 1 ||
      napi_get_value_int32(env, value, &descriptor_setter_value) != napi_ok) return NULL;
  return NULL;
}

static napi_value counter_constructor(napi_env env, napi_callback_info info) {
  napi_value argument, this_arg, initial_value, new_target, target_name;
  size_t argc = 1;
  void* data = NULL;
  int32_t value = 0;
  char target_name_text[64] = {0};
  if (napi_get_cb_info(env, info, &argc, &argument, &this_arg, &data) != napi_ok ||
      this_arg == NULL || data != &class_constructor_offset ||
      napi_get_new_target(env, info, &new_target) != napi_ok) return NULL;
  counter_new_target_seen = new_target != NULL;
  counter_child_new_target_seen = false;
  if (new_target != NULL &&
      napi_get_named_property(env, new_target, "name", &target_name) == napi_ok) {
    size_t target_name_length = 0;
    if (napi_get_value_string_utf8(env, target_name, target_name_text,
                                   sizeof(target_name_text),
                                   &target_name_length) == napi_ok) {
      counter_child_new_target_seen =
          strcmp(target_name_text, "CounterChild") == 0;
    }
  }
  if (argc > 0 && napi_get_value_int32(env, argument, &value) != napi_ok) return NULL;
  if (napi_create_int32(env, value + *(int*)data, &initial_value) != napi_ok ||
      napi_set_named_property(env, this_arg, "value", initial_value) != napi_ok) return NULL;
  return NULL;
}

static napi_value target_probe(napi_env env, napi_callback_info info) {
  napi_value new_target;
  if (napi_get_new_target(env, info, &new_target) != napi_ok) return NULL;
  if (new_target == NULL) target_call_count++;
  else target_construct_count++;
  return NULL;
}

static napi_value target_counts(napi_env env, napi_callback_info info) {
  napi_value result, field;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_int32(env, target_call_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "calls", field) != napi_ok ||
      napi_create_int32(env, target_construct_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "constructs", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value strict_equal_probe(napi_env env, napi_callback_info info) {
  napi_value args[2], result;
  size_t argc = 2;
  bool equal = false;
  if (napi_get_cb_info(env, info, &argc, args, NULL, NULL) != napi_ok || argc < 2 ||
      napi_strict_equals(env, args[0], args[1], &equal) != napi_ok ||
      napi_get_boolean(env, equal, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_new_target_info(napi_env env, napi_callback_info info) {
  napi_value result, field;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, counter_new_target_seen, &field) != napi_ok ||
      napi_set_named_property(env, result, "seen", field) != napi_ok ||
      napi_get_boolean(env, counter_child_new_target_seen, &field) != napi_ok ||
      napi_set_named_property(env, result, "child", field) != napi_ok)
    return NULL;
  return result;
}

static napi_value counter_increment(napi_env env, napi_callback_info info) {
  napi_value this_arg, value, result;
  size_t argc = 0;
  int32_t current;
  if (napi_get_cb_info(env, info, &argc, NULL, &this_arg, NULL) != napi_ok ||
      napi_get_named_property(env, this_arg, "value", &value) != napi_ok ||
      napi_get_value_int32(env, value, &current) != napi_ok ||
      napi_create_int32(env, current + 1, &result) != napi_ok ||
      napi_set_named_property(env, this_arg, "value", result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_method(napi_env env, napi_callback_info info) {
  napi_value this_arg, base_value, result;
  size_t argc = 0;
  int32_t base;
  if (napi_get_cb_info(env, info, &argc, NULL, &this_arg, NULL) != napi_ok ||
      napi_get_named_property(env, this_arg, "baseValue", &base_value) != napi_ok ||
      napi_get_value_int32(env, base_value, &base) != napi_ok ||
      napi_create_int32(env, base + 99, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_getter(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, counter_static_offset, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_read_only_getter(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_create_int32(env, 21, &result) != napi_ok) return NULL;
  return result;
}

static napi_value counter_static_setter(napi_env env, napi_callback_info info) {
  napi_value value;
  size_t argc = 1;
  if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok || argc < 1 ||
      napi_get_value_int32(env, value, &counter_static_offset) != napi_ok) return NULL;
  return NULL;
}

static napi_value symbol_probe(napi_env env, napi_callback_info info) {
  napi_value description, key, other_key, no_description, object;
  napi_value value, actual, names, result, field;
  napi_valuetype no_description_type;
  uint32_t string_key_count = 0;
  bool has_key = false, has_other_key = true;
  (void)info;
  if (napi_create_string_utf8(env, "native-symbol", NAPI_AUTO_LENGTH, &description) != napi_ok ||
      napi_create_symbol(env, description, &key) != napi_ok ||
      napi_create_symbol(env, description, &other_key) != napi_ok ||
      napi_create_symbol(env, NULL, &no_description) != napi_ok ||
      napi_typeof(env, no_description, &no_description_type) != napi_ok ||
      napi_create_object(env, &object) != napi_ok ||
      napi_create_int32(env, 42, &value) != napi_ok ||
      napi_set_property(env, object, key, value) != napi_ok ||
      napi_get_property(env, object, key, &actual) != napi_ok ||
      napi_has_own_property(env, object, key, &has_key) != napi_ok ||
      napi_has_own_property(env, object, other_key, &has_other_key) != napi_ok ||
      napi_get_property_names(env, object, &names) != napi_ok ||
      napi_get_array_length(env, names, &string_key_count) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "value", actual) != napi_ok ||
      napi_get_boolean(env, has_key, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasKey", field) != napi_ok ||
      napi_get_boolean(env, has_other_key, &field) != napi_ok ||
      napi_set_named_property(env, result, "hasOtherKey", field) != napi_ok ||
      napi_get_boolean(env, no_description_type == napi_symbol, &field) != napi_ok ||
      napi_set_named_property(env, result, "noDescriptionIsSymbol", field) != napi_ok ||
      napi_create_uint32(env, string_key_count, &field) != napi_ok ||
      napi_set_named_property(env, result, "stringKeyCount", field) != napi_ok) return NULL;
  return result;
}

static napi_value typedarray_probe(napi_env env, napi_callback_info info) {
  napi_value buffer, typed, typed_buffer, view, view_buffer, result, field;
  void* bytes = NULL;
  void* typed_bytes = NULL;
  void* view_bytes = NULL;
  size_t buffer_length = 0, typed_length = 0, typed_offset = 0;
  size_t view_length = 0, view_offset = 0;
  napi_typedarray_type typed_kind = napi_int8_array;
  bool is_arraybuffer = false, is_typedarray = false, is_dataview = false;
  (void)info;
  if (napi_create_arraybuffer(env, 8, &bytes, &buffer) != napi_ok || bytes == NULL ||
      napi_get_arraybuffer_info(env, buffer, &bytes, &buffer_length) != napi_ok ||
      buffer_length != 8 ||
      napi_is_arraybuffer(env, buffer, &is_arraybuffer) != napi_ok || !is_arraybuffer) return NULL;
  for (size_t i = 0; i < buffer_length; i++) ((uint8_t*)bytes)[i] = (uint8_t)(10 + i);
  if (napi_create_typedarray(env, napi_uint16_array, 2, buffer, 2, &typed) != napi_ok ||
      napi_get_typedarray_info(env, typed, &typed_kind, &typed_length, &typed_bytes,
                               &typed_buffer, &typed_offset) != napi_ok ||
      typed_kind != napi_uint16_array || typed_length != 2 || typed_offset != 2 ||
      typed_bytes == NULL ||
      napi_is_typedarray(env, typed, &is_typedarray) != napi_ok || !is_typedarray ||
      napi_create_dataview(env, 3, buffer, 4, &view) != napi_ok ||
      napi_get_dataview_info(env, view, &view_length, &view_bytes, &view_buffer,
                             &view_offset) != napi_ok ||
      view_length != 3 || view_offset != 4 || view_bytes == NULL ||
      napi_is_dataview(env, view, &is_dataview) != napi_ok || !is_dataview) return NULL;
  ((uint8_t*)typed_bytes)[1] = 55;
  ((uint8_t*)view_bytes)[0] = 77;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "buffer", buffer) != napi_ok ||
      napi_set_named_property(env, result, "typed", typed) != napi_ok ||
      napi_set_named_property(env, result, "view", view) != napi_ok ||
      napi_get_boolean(env, is_arraybuffer, &field) != napi_ok ||
      napi_set_named_property(env, result, "isArrayBuffer", field) != napi_ok ||
      napi_get_boolean(env, is_typedarray, &field) != napi_ok ||
      napi_set_named_property(env, result, "isTypedArray", field) != napi_ok ||
      napi_get_boolean(env, is_dataview, &field) != napi_ok ||
      napi_set_named_property(env, result, "isDataView", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)typed_kind, &field) != napi_ok ||
      napi_set_named_property(env, result, "typedKind", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)typed_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "typedLength", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)typed_offset, &field) != napi_ok ||
      napi_set_named_property(env, result, "typedOffset", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)view_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "viewLength", field) != napi_ok ||
      napi_create_uint32(env, (uint32_t)view_offset, &field) != napi_ok ||
      napi_set_named_property(env, result, "viewOffset", field) != napi_ok) return NULL;
  return result;
}

static napi_value array_probe(napi_env env, napi_callback_info info) {
  napi_value array, empty, result, value, field;
  uint32_t length = 0, empty_length = 0;
  bool is_array = false, first_present = true, second_present = false;
  bool hole_present = true, read_value = false;
  (void)info;
  if (napi_create_array_with_length(env, 3, &array) != napi_ok ||
      napi_create_array(env, &empty) != napi_ok ||
      napi_get_boolean(env, true, &value) != napi_ok ||
      napi_set_element(env, array, 1, value) != napi_ok ||
      napi_set_element(env, array, 4, value) != napi_ok ||
      napi_get_array_length(env, array, &length) != napi_ok ||
      napi_get_array_length(env, empty, &empty_length) != napi_ok ||
      napi_is_array(env, array, &is_array) != napi_ok ||
      napi_has_element(env, array, 0, &first_present) != napi_ok ||
      napi_has_element(env, array, 1, &second_present) != napi_ok ||
      napi_has_element(env, array, 2, &hole_present) != napi_ok ||
      napi_get_element(env, array, 1, &value) != napi_ok ||
      napi_get_value_bool(env, value, &read_value) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_get_boolean(env, is_array, &field) != napi_ok ||
      napi_set_named_property(env, result, "isArray", field) != napi_ok ||
      napi_create_uint32(env, length, &field) != napi_ok ||
      napi_set_named_property(env, result, "length", field) != napi_ok ||
      napi_create_uint32(env, empty_length, &field) != napi_ok ||
      napi_set_named_property(env, result, "emptyLength", field) != napi_ok ||
      napi_get_boolean(env, first_present, &field) != napi_ok ||
      napi_set_named_property(env, result, "firstPresent", field) != napi_ok ||
      napi_get_boolean(env, second_present, &field) != napi_ok ||
      napi_set_named_property(env, result, "secondPresent", field) != napi_ok ||
      napi_get_boolean(env, hole_present, &field) != napi_ok ||
      napi_set_named_property(env, result, "holePresent", field) != napi_ok ||
      napi_get_boolean(env, read_value, &field) != napi_ok ||
      napi_set_named_property(env, result, "value", field) != napi_ok) return NULL;
  return result;
}

static napi_value get_prototype_probe(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value object, prototype;
  if (napi_get_cb_info(env, info, &argc, &object, NULL, NULL) != napi_ok || argc != 1 ||
      napi_get_prototype(env, object, &prototype) != napi_ok) return NULL;
  return prototype;
}

static napi_value instanceof_probe(napi_env env, napi_callback_info info) {
  size_t argc = 2;
  napi_value argv[2], result;
  bool is_instance = false;
  if (napi_get_cb_info(env, info, &argc, argv, NULL, NULL) != napi_ok || argc != 2 ||
      napi_instanceof(env, argv[0], argv[1], &is_instance) != napi_ok ||
      napi_get_boolean(env, is_instance, &result) != napi_ok) return NULL;
  return result;
}

static napi_value returns_undefined(napi_env env, napi_callback_info info) {
  (void)env;
  (void)info;
  return NULL;
}

static napi_value create_errors(napi_env env, napi_callback_info info) {
  napi_value result, message, code, error, type_error, range_error;
  (void)info;
  if (napi_create_object(env, &result) != napi_ok ||
      napi_create_string_utf8(env, "created", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_string_utf8(env, "E_CREATED", NAPI_AUTO_LENGTH, &code) != napi_ok ||
      napi_create_error(env, code, message, &error) != napi_ok ||
      napi_set_named_property(env, result, "error", error) != napi_ok ||
      napi_create_type_error(env, code, message, &type_error) != napi_ok ||
      napi_set_named_property(env, result, "typeError", type_error) != napi_ok ||
      napi_create_range_error(env, code, message, &range_error) != napi_ok ||
      napi_set_named_property(env, result, "rangeError", range_error) != napi_ok) return NULL;
  return result;
}

static napi_value throw_type_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_type_error(env, "E_TYPE", "type failure");
  return NULL;
}

static napi_value throw_range_error(napi_env env, napi_callback_info info) {
  (void)info;
  napi_throw_range_error(env, NULL, "range failure");
  return NULL;
}

static napi_value throw_created_error(napi_env env, napi_callback_info info) {
  napi_value message, error;
  (void)info;
  if (napi_create_string_utf8(env, "thrown", NAPI_AUTO_LENGTH, &message) != napi_ok ||
      napi_create_type_error(env, NULL, message, &error) != napi_ok ||
      napi_throw(env, error) != napi_ok) return NULL;
  return NULL;
}

static napi_value throw_and_clear(napi_env env, napi_callback_info info) {
  napi_value error;
  bool pending = false, is_error = false;
  (void)info;
  if (napi_throw_error(env, "E_CLEARED", "cleared failure") != napi_ok ||
      napi_is_exception_pending(env, &pending) != napi_ok || !pending ||
      napi_get_and_clear_last_exception(env, &error) != napi_ok ||
      napi_is_exception_pending(env, &pending) != napi_ok || pending ||
      napi_is_error(env, error, &is_error) != napi_ok || !is_error) return NULL;
  return error;
}

static napi_value reference_probe(napi_env env, napi_callback_info info) {
  napi_value value, result, field;
  uint32_t count_after_unref = 0, count_after_ref = 0;
  (void)info;
  if (napi_reference_unref(env, persistent_values, &count_after_unref) != napi_ok ||
      napi_get_reference_value(env, persistent_values, &value) != napi_ok || value == NULL ||
      napi_reference_ref(env, persistent_values, &count_after_ref) != napi_ok ||
      napi_create_object(env, &result) != napi_ok ||
      napi_set_named_property(env, result, "value", value) != napi_ok ||
      napi_create_uint32(env, count_after_unref, &field) != napi_ok ||
      napi_set_named_property(env, result, "countAfterUnref", field) != napi_ok ||
      napi_create_uint32(env, count_after_ref, &field) != napi_ok ||
      napi_set_named_property(env, result, "countAfterRef", field) != napi_ok) return NULL;
  return result;
}

static napi_value release_reference(napi_env env, napi_callback_info info) {
  napi_value result;
  (void)info;
  if (napi_delete_reference(env, persistent_values) != napi_ok ||
      napi_delete_reference(env, wrapped_object_reference) != napi_ok ||
      napi_get_boolean(env, true, &result) != napi_ok) return NULL;
  return result;
}

static napi_value wrap_probe(napi_env env, napi_callback_info info) {
  napi_value object, result;
  void* data = NULL;
  (void)info;
  if (napi_get_reference_value(env, wrapped_object_reference, &object) != napi_ok ||
      napi_unwrap(env, object, &data) != napi_ok || data != wrapped_native_data ||
      napi_create_string_utf8(env, (const char*)data, NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  return result;
}

static napi_value remove_wrap_probe(napi_env env, napi_callback_info info) {
  napi_value object, result;
  void* data = NULL;
  (void)info;
  if (napi_get_reference_value(env, removable_object, &object) != napi_ok ||
      napi_remove_wrap(env, object, &data) != napi_ok || data != removable_native_data ||
      napi_create_string_utf8(env, (const char*)data, NAPI_AUTO_LENGTH, &result) != napi_ok) return NULL;
  return result;
}

static napi_value duplicate_wrap_status(napi_env env, napi_callback_info info) {
  napi_value object, result;
  napi_status status;
  (void)info;
  if (napi_get_reference_value(env, wrapped_object_reference, &object) != napi_ok) return NULL;
  status = napi_wrap(env, object, wrapped_native_data, finalize_probe, NULL, NULL);
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value external_probe(napi_env env, napi_callback_info info) {
  napi_value external, ordinary, result;
  napi_valuetype type;
  void* data = NULL;
  void* invalid_data = NULL;
  (void)info;
  if (napi_get_reference_value(env, external_value_reference, &external) != napi_ok ||
      napi_get_value_external(env, external, &data) != napi_ok ||
      data != external_native_data ||
      napi_typeof(env, external, &type) != napi_ok || type != napi_external ||
      napi_create_object(env, &ordinary) != napi_ok ||
      napi_get_value_external(env, ordinary, &invalid_data) != napi_invalid_arg ||
      napi_get_boolean(env, true, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value external_property_probe(napi_env env, napi_callback_info info) {
  napi_value external, ignored, property, result;
  napi_valuetype type;
  (void)info;
  if (napi_get_reference_value(env, external_value_reference, &external) != napi_ok ||
      napi_create_object(env, &property) != napi_ok ||
      napi_set_named_property(env, external, "ignored", property) != napi_ok ||
      napi_get_named_property(env, external, "ignored", &ignored) != napi_ok ||
      napi_typeof(env, ignored, &type) != napi_ok || type != napi_undefined ||
      napi_get_boolean(env, true, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value external_memory_probe(napi_env env, napi_callback_info info) {
  int64_t first = 0, second = 0;
  napi_value result;
  (void)info;
  if (napi_adjust_external_memory(env, INT64_C(65536), &first) != napi_ok ||
      napi_adjust_external_memory(env, -INT64_C(4096), &second) != napi_ok ||
      napi_create_int64(env, second - first, &result) != napi_ok)
    return NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_handle_scope scope;
  napi_value scratch, function, metadata, version, values, field, external;
  uint32_t supported_api_version = 0;
  napi_deferred initialized_deferred;
  napi_value initialized_promise, initialized_promise_value;
  bool initialized_is_promise = false;
  napi_value descriptor_value, descriptor_symbol, descriptor_symbol_description;
  napi_value descriptor_symbol_value;
  napi_value global, global_key, global_object_constructor;
  void* current_instance_data = NULL;
  napi_valuetype global_object_type;
  bool global_object_own = false;
  napi_property_descriptor defined_properties[4] = {
      { .utf8name = "definedMethod", .method = defined_method,
        .attributes = napi_default },
      { .utf8name = "definedValue", .getter = defined_getter,
        .setter = defined_setter, .attributes = napi_enumerable },
      { .utf8name = "definedConstant", .value = NULL,
        .attributes = napi_writable | napi_enumerable | napi_configurable },
      { .name = NULL, .value = NULL,
        .attributes = napi_writable | napi_enumerable | napi_configurable },
  };
  napi_property_descriptor counter_methods[] = {
      { .utf8name = "increment", .method = counter_increment,
        .attributes = napi_default },
      { .utf8name = "constant", .method = counter_static_method,
        .attributes = napi_static },
      { .utf8name = "baseValue", .value = NULL,
        .attributes = napi_static | napi_writable | napi_enumerable | napi_configurable },
      { .utf8name = "offset", .getter = counter_static_getter,
        .setter = counter_static_setter,
        .attributes = napi_static | napi_enumerable },
      { .utf8name = "readOnly", .getter = counter_static_read_only_getter,
        .attributes = napi_static | napi_enumerable },
  };
  napi_value counter_class, counter_base_value;
  int32_t checked_version = 0;
  if (napi_get_version(env, &supported_api_version) != napi_ok ||
      supported_api_version < NAPI_VERSION ||
      napi_get_version(env, NULL) != napi_invalid_arg ||
      napi_get_boolean(env, supported_api_version >= NAPI_VERSION, &field) != napi_ok ||
      napi_set_named_property(env, exports, "supportsNapiV7", field) != napi_ok)
    return NULL;
  if (napi_get_instance_data(env, &current_instance_data) != napi_ok ||
      current_instance_data != NULL ||
      napi_set_instance_data(env, &instance_data_first_marker,
                             finalize_instance_data, NULL) != napi_ok ||
      napi_set_instance_data(env, &instance_data_second_marker,
                             finalize_instance_data,
                             &instance_data_finalize_hint) != napi_ok ||
      napi_get_instance_data(env, &current_instance_data) != napi_ok ||
      current_instance_data != &instance_data_second_marker)
    return NULL;
  if (napi_create_external(env, external_native_data, finalize_external_probe,
                           &external_finalize_hint, &external) != napi_ok ||
      napi_create_reference(env, external, 1, &external_value_reference) != napi_ok ||
      napi_set_named_property(env, exports, "external", external) != napi_ok)
    return NULL;
  cleanup_env = env;
  if (napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)1) != napi_ok ||
      napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)2) != napi_ok ||
      napi_add_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)3) != napi_ok ||
      napi_remove_env_cleanup_hook(env, cleanup_probe, (void*)(intptr_t)2) != napi_ok ||
      napi_add_env_cleanup_hook(env, cleanup_remove_other,
                                (void*)(intptr_t)1) != napi_ok)
    return NULL;
  if (napi_create_int32(env, 7, &descriptor_value) != napi_ok ||
      napi_create_string_utf8(env, "descriptor", NAPI_AUTO_LENGTH,
                              &descriptor_symbol_description) != napi_ok ||
      napi_create_symbol(env, descriptor_symbol_description,
                         &descriptor_symbol) != napi_ok ||
      napi_create_int32(env, 17, &descriptor_symbol_value) != napi_ok ||
      napi_create_int32(env, 6, &counter_base_value) != napi_ok) return NULL;
  defined_properties[2].value = descriptor_value;
  defined_properties[3].name = descriptor_symbol;
  defined_properties[3].value = descriptor_symbol_value;
      counter_methods[2].value = counter_base_value;
  if (napi_get_global(env, &global) != napi_ok ||
      napi_create_string_utf8(env, "Object", NAPI_AUTO_LENGTH, &global_key) != napi_ok ||
      napi_get_named_property(env, global, "Object", &global_object_constructor) != napi_ok ||
      napi_typeof(env, global_object_constructor, &global_object_type) != napi_ok ||
      global_object_type != napi_function ||
      napi_has_own_property(env, global, global_key, &global_object_own) != napi_ok ||
      !global_object_own ||
      napi_open_handle_scope(env, &scope) != napi_ok ||
      napi_create_object(env, &scratch) != napi_ok ||
      napi_close_handle_scope(env, scope) != napi_ok) return NULL;
  if (napi_is_promise(env, exports, &initialized_is_promise) != napi_ok ||
      initialized_is_promise ||
      napi_create_promise(env, &initialized_deferred, &initialized_promise) != napi_ok ||
      napi_is_promise(env, initialized_promise, &initialized_is_promise) != napi_ok ||
      !initialized_is_promise ||
      napi_create_string_utf8(env, "resolved-during-init", NAPI_AUTO_LENGTH,
                              &initialized_promise_value) != napi_ok ||
      napi_resolve_deferred(env, initialized_deferred, initialized_promise_value) != napi_ok ||
      napi_set_named_property(env, exports, "initializedPromise", initialized_promise) != napi_ok)
    return NULL;
  if (napi_set_named_property(env, exports, "descriptorSymbol", descriptor_symbol) != napi_ok ||
      napi_define_properties(env, exports, 4, defined_properties) != napi_ok) return NULL;
      if (napi_define_class(env, "Counter", NAPI_AUTO_LENGTH, counter_constructor,
                        &class_constructor_offset, 5, counter_methods,
                        &counter_class) != napi_ok ||
      napi_set_named_property(env, exports, "Counter", counter_class) != napi_ok) return NULL;
  if (napi_create_function(env, "add", NAPI_AUTO_LENGTH, add, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "add", function) != napi_ok ||
      napi_create_function(env, "externalProbe", NAPI_AUTO_LENGTH,
                           external_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalProbe", function) != napi_ok ||
      napi_create_function(env, "makeExternalArrayBuffer", NAPI_AUTO_LENGTH,
                           make_external_arraybuffer, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "makeExternalArrayBuffer", function) != napi_ok ||
      napi_create_function(env, "checkExternalArrayBuffer", NAPI_AUTO_LENGTH,
                           check_external_arraybuffer, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "checkExternalArrayBuffer", function) != napi_ok ||
      napi_create_function(env, "arraybufferDetachmentProbe", NAPI_AUTO_LENGTH,
                           arraybuffer_detachment_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "arraybufferDetachmentProbe", function) != napi_ok ||
      napi_create_function(env, "makeExternalBuffer", NAPI_AUTO_LENGTH,
                           make_external_buffer, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "makeExternalBuffer", function) != napi_ok ||
      napi_create_function(env, "checkExternalBuffer", NAPI_AUTO_LENGTH,
                           check_external_buffer, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "checkExternalBuffer", function) != napi_ok ||
      napi_create_function(env, "asyncContextProbe", NAPI_AUTO_LENGTH,
                           async_context_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "asyncContextProbe", function) != napi_ok ||
      napi_create_function(env, "dateProbe", NAPI_AUTO_LENGTH,
                           date_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "dateProbe", function) != napi_ok ||
      napi_create_function(env, "bigintProbe", NAPI_AUTO_LENGTH,
                           bigint_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "bigintProbe", function) != napi_ok ||
      napi_create_function(env, "instanceDataProbe", NAPI_AUTO_LENGTH,
                           instance_data_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "instanceDataProbe", function) != napi_ok ||
      napi_create_function(env, "propertyNamesProbe", NAPI_AUTO_LENGTH,
                           property_names_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "propertyNamesProbe", function) != napi_ok ||
      napi_create_function(env, "proxyPropertyNamesProbe", NAPI_AUTO_LENGTH,
                           proxy_property_names_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "proxyPropertyNamesProbe", function) != napi_ok ||
      napi_create_function(env, "globalPropertyNamesProbe", NAPI_AUTO_LENGTH,
                           global_property_names_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "globalPropertyNamesProbe", function) != napi_ok ||
      napi_create_function(env, "externalPropertyProbe", NAPI_AUTO_LENGTH,
                           external_property_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalPropertyProbe", function) != napi_ok ||
      napi_create_function(env, "externalMemoryProbe", NAPI_AUTO_LENGTH,
                           external_memory_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "externalMemoryProbe", function) != napi_ok ||
      napi_create_function(env, "coerceToBoolean", NAPI_AUTO_LENGTH,
                           coerce_to_bool_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToBoolean", function) != napi_ok ||
      napi_create_function(env, "coerceToNumber", NAPI_AUTO_LENGTH,
                           coerce_to_number_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToNumber", function) != napi_ok ||
      napi_create_function(env, "coerceToString", NAPI_AUTO_LENGTH,
                           coerce_to_string_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToString", function) != napi_ok ||
      napi_create_function(env, "coerceToObject", NAPI_AUTO_LENGTH,
                           coerce_to_object_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "coerceToObject", function) != napi_ok ||
      napi_create_function(env, "deleteElementProbe", NAPI_AUTO_LENGTH,
                           delete_element_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "deleteElementProbe", function) != napi_ok ||
      napi_create_function(env, "escapableScopeProbe", NAPI_AUTO_LENGTH,
                           escapable_scope_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "escapableScopeProbe", function) != napi_ok ||
      napi_create_function(env, "runScriptProbe", NAPI_AUTO_LENGTH,
                           run_script_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "runScriptProbe", function) != napi_ok ||
      napi_create_function(env, "stringEncodingProbe", NAPI_AUTO_LENGTH,
                           string_encoding_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "stringEncodingProbe", function) != napi_ok ||
      napi_create_function(env, "utf16Probe", NAPI_AUTO_LENGTH,
                           utf16_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "utf16Probe", function) != napi_ok ||
      napi_create_function(env, "invalidUtf16Status", NAPI_AUTO_LENGTH,
                           invalid_utf16_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidUtf16Status", function) != napi_ok ||
      napi_create_object(env, &metadata) != napi_ok ||
      napi_create_int32(env, 1, &version) != napi_ok ||
      napi_set_named_property(env, metadata, "version", version) != napi_ok ||
      napi_get_named_property(env, metadata, "version", &field) != napi_ok ||
      napi_get_value_int32(env, field, &checked_version) != napi_ok ||
      checked_version != 1 ||
      napi_set_named_property(env, exports, "metadata", metadata) != napi_ok) return NULL;
  if (napi_create_function(env, "targetProbe", NAPI_AUTO_LENGTH, target_probe,
                           NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "targetProbe", function) != napi_ok ||
      napi_create_function(env, "targetCounts", NAPI_AUTO_LENGTH, target_counts,
                           NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "targetCounts", function) != napi_ok ||
      napi_create_function(env, "strictEqualProbe", NAPI_AUTO_LENGTH,
                           strict_equal_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "strictEqualProbe", function) != napi_ok ||
      napi_create_function(env, "counterNewTargetInfo", NAPI_AUTO_LENGTH,
                           counter_new_target_info, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "counterNewTargetInfo", function) != napi_ok)
    return NULL;
  if (napi_create_object(env, &values) != napi_ok ||
      napi_get_boolean(env, true, &field) != napi_ok ||
      napi_set_named_property(env, values, "truth", field) != napi_ok ||
      napi_get_null(env, &field) != napi_ok ||
      napi_set_named_property(env, values, "nothing", field) != napi_ok ||
      napi_get_undefined(env, &field) != napi_ok ||
      napi_set_named_property(env, values, "missing", field) != napi_ok ||
      napi_create_string_utf8(env, "Node-API ✓", NAPI_AUTO_LENGTH, &field) != napi_ok ||
      napi_set_named_property(env, values, "greeting", field) != napi_ok ||
      napi_create_double(env, 1.25, &field) != napi_ok ||
      napi_set_named_property(env, values, "fraction", field) != napi_ok ||
      napi_create_uint32(env, UINT32_MAX, &field) != napi_ok ||
      napi_set_named_property(env, values, "maxUint32", field) != napi_ok ||
      napi_create_int64(env, INT64_C(2147483648), &field) != napi_ok ||
      napi_set_named_property(env, values, "int64", field) != napi_ok ||
      napi_set_named_property(env, exports, "values", values) != napi_ok ||
      napi_wrap(env, values, wrapped_native_data, finalize_probe, NULL, &wrapped_object_reference) != napi_ok ||
      napi_create_reference(env, values, 1, &persistent_values) != napi_ok ||
      napi_create_object(env, &field) != napi_ok ||
      napi_wrap(env, field, removable_native_data, finalize_probe, NULL, NULL) != napi_ok ||
      napi_create_reference(env, field, 1, &removable_object) != napi_ok ||
      napi_create_function(env, "roundTrip", NAPI_AUTO_LENGTH, round_trip, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "roundTrip", function) != napi_ok ||
      napi_create_function(env, "int64ConversionProbe", NAPI_AUTO_LENGTH,
                           int64_conversion_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "int64ConversionProbe", function) != napi_ok ||
      napi_create_function(env, "getPrototype", NAPI_AUTO_LENGTH,
                           get_prototype_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "getPrototype", function) != napi_ok ||
      napi_create_function(env, "instanceofProbe", NAPI_AUTO_LENGTH,
                           instanceof_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "instanceofProbe", function) != napi_ok ||
      napi_create_function(env, "arrayProbe", NAPI_AUTO_LENGTH, array_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "arrayProbe", function) != napi_ok ||
      napi_create_function(env, "returnsUndefined", NAPI_AUTO_LENGTH, returns_undefined, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "returnsUndefined", function) != napi_ok ||
      napi_create_function(env, "createErrors", NAPI_AUTO_LENGTH, create_errors, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "createErrors", function) != napi_ok ||
      napi_create_function(env, "throwTypeError", NAPI_AUTO_LENGTH, throw_type_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwTypeError", function) != napi_ok ||
      napi_create_function(env, "throwRangeError", NAPI_AUTO_LENGTH, throw_range_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwRangeError", function) != napi_ok ||
      napi_create_function(env, "throwCreatedError", NAPI_AUTO_LENGTH, throw_created_error, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwCreatedError", function) != napi_ok ||
      napi_create_function(env, "throwAndClear", NAPI_AUTO_LENGTH, throw_and_clear, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "throwAndClear", function) != napi_ok ||
      napi_create_function(env, "referenceProbe", NAPI_AUTO_LENGTH, reference_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "referenceProbe", function) != napi_ok ||
      napi_create_function(env, "releaseReference", NAPI_AUTO_LENGTH, release_reference, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "releaseReference", function) != napi_ok ||
      napi_create_function(env, "wrapProbe", NAPI_AUTO_LENGTH, wrap_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "wrapProbe", function) != napi_ok ||
      napi_create_function(env, "removeWrapProbe", NAPI_AUTO_LENGTH, remove_wrap_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "removeWrapProbe", function) != napi_ok ||
      napi_create_function(env, "duplicateWrapStatus", NAPI_AUTO_LENGTH, duplicate_wrap_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "duplicateWrapStatus", function) != napi_ok ||
      napi_create_function(env, "bufferProbe", NAPI_AUTO_LENGTH, buffer_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "bufferProbe", function) != napi_ok ||
      napi_create_function(env, "typedArrayProbe", NAPI_AUTO_LENGTH, typedarray_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "typedArrayProbe", function) != napi_ok ||
      napi_create_function(env, "invalidTypedArray", NAPI_AUTO_LENGTH, invalid_typedarray, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidTypedArray", function) != napi_ok ||
      napi_create_function(env, "callGuest", NAPI_AUTO_LENGTH, call_guest, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "callGuest", function) != napi_ok ||
      napi_create_function(env, "constructGuest", NAPI_AUTO_LENGTH, construct_guest, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "constructGuest", function) != napi_ok ||
      napi_create_function(env, "resolvedPromise", NAPI_AUTO_LENGTH, resolved_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "resolvedPromise", function) != napi_ok ||
      napi_create_function(env, "runAsync", NAPI_AUTO_LENGTH, run_async_work, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "runAsync", function) != napi_ok ||
      napi_create_function(env, "runThreadsafe", NAPI_AUTO_LENGTH, run_threadsafe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "runThreadsafe", function) != napi_ok ||
      napi_create_function(env, "probeThreadsafeQueue", NAPI_AUTO_LENGTH, probe_threadsafe_queue, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "probeThreadsafeQueue", function) != napi_ok ||
      napi_create_function(env, "probeThreadsafeAbort", NAPI_AUTO_LENGTH, probe_threadsafe_abort, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "probeThreadsafeAbort", function) != napi_ok ||
      napi_create_function(env, "rejectedPromise", NAPI_AUTO_LENGTH, rejected_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "rejectedPromise", function) != napi_ok ||
      napi_create_function(env, "pendingPromise", NAPI_AUTO_LENGTH, pending_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "pendingPromise", function) != napi_ok ||
      napi_create_function(env, "resolvePendingPromise", NAPI_AUTO_LENGTH, resolve_pending_promise, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "resolvePendingPromise", function) != napi_ok ||
      napi_create_function(env, "propertyProbe", NAPI_AUTO_LENGTH, property_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "propertyProbe", function) != napi_ok ||
      napi_create_function(env, "globalProbe", NAPI_AUTO_LENGTH, global_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "globalProbe", function) != napi_ok ||
      napi_create_function(env, "symbolProbe", NAPI_AUTO_LENGTH, symbol_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "symbolProbe", function) != napi_ok ||
      napi_create_function(env, "invalidEnvironment", NAPI_AUTO_LENGTH, invalid_environment, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "invalidEnvironment", function) != napi_ok ||
      napi_create_function(env, "cleanupMisuseStatus", NAPI_AUTO_LENGTH, cleanup_misuse_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "cleanupMisuseStatus", function) != napi_ok ||
      napi_create_function(env, "errorInfoProbe", NAPI_AUTO_LENGTH, error_info_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "errorInfoProbe", function) != napi_ok) return NULL;
  return exports;
}
