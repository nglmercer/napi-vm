#include <node_api.h>

static napi_ref weak_object_reference;

static napi_status set_int32(napi_env env, napi_value target, const char* name,
                             int32_t value) {
  napi_value result;
  napi_status status = napi_create_int32(env, value, &result);
  if (status != napi_ok) return status;
  return napi_set_named_property(env, target, name, result);
}

static napi_status set_boolean(napi_env env, napi_value target,
                               const char* name, bool value) {
  napi_value result;
  napi_status status = napi_get_boolean(env, value, &result);
  if (status != napi_ok) return status;
  return napi_set_named_property(env, target, name, result);
}

static napi_value reference_probe(napi_env env, napi_callback_info info) {
  (void)info;
  napi_value report;
  napi_value primitive;
  napi_ref strong_ref = NULL;
  napi_ref zero_ref = NULL;
  uint32_t count = 0;
  napi_status create_strong_status;
  napi_status initial_value_status = napi_generic_failure;
  napi_status unref_status = napi_generic_failure;
  napi_status released_value_status = napi_generic_failure;
  napi_status ref_after_release_status = napi_generic_failure;
  napi_status value_after_release_ref_status = napi_generic_failure;
  napi_status create_zero_status;
  napi_status zero_value_status = napi_generic_failure;
  napi_value referenced_value = NULL;
  uint32_t count_after_unref = 0;
  bool initial_value_present = false;
  bool released_value_is_null = false;
  bool value_after_release_ref_is_null = false;
  bool zero_value_is_null = false;

  if (napi_create_object(env, &report) != napi_ok ||
      napi_create_int32(env, 37, &primitive) != napi_ok)
    return NULL;

  create_strong_status = napi_create_reference(env, primitive, 1, &strong_ref);
  if (create_strong_status == napi_ok) {
    initial_value_status = napi_get_reference_value(env, strong_ref,
                                                    &referenced_value);
    if (initial_value_status == napi_ok && referenced_value != NULL) {
      initial_value_present = true;
      unref_status = napi_reference_unref(env, strong_ref, &count);
      count_after_unref = count;
      referenced_value = NULL;
      released_value_status = napi_get_reference_value(env, strong_ref,
                                                       &referenced_value);
      released_value_is_null = released_value_status == napi_ok &&
                               referenced_value == NULL;
      ref_after_release_status = napi_reference_ref(env, strong_ref, &count);
      referenced_value = NULL;
      value_after_release_ref_status = napi_get_reference_value(
          env, strong_ref, &referenced_value);
      value_after_release_ref_is_null =
          value_after_release_ref_status == napi_ok && referenced_value == NULL;
    }
    napi_delete_reference(env, strong_ref);
  }

  create_zero_status = napi_create_reference(env, primitive, 0, &zero_ref);
  if (create_zero_status == napi_ok) {
    referenced_value = NULL;
    zero_value_status = napi_get_reference_value(env, zero_ref,
                                                 &referenced_value);
    zero_value_is_null = zero_value_status == napi_ok && referenced_value == NULL;
    napi_delete_reference(env, zero_ref);
  }

  if (set_int32(env, report, "createStrongStatus", create_strong_status) != napi_ok ||
      set_int32(env, report, "initialValueStatus", initial_value_status) != napi_ok ||
      set_int32(env, report, "unrefStatus", unref_status) != napi_ok ||
      set_int32(env, report, "countAfterUnref", (int32_t)count_after_unref) != napi_ok ||
      set_int32(env, report, "releasedValueStatus", released_value_status) != napi_ok ||
      set_int32(env, report, "refAfterReleaseStatus", ref_after_release_status) != napi_ok ||
      set_int32(env, report, "valueAfterReleaseRefStatus",
                value_after_release_ref_status) != napi_ok ||
      set_int32(env, report, "createZeroStatus", create_zero_status) != napi_ok ||
      set_int32(env, report, "zeroValueStatus", zero_value_status) != napi_ok ||
      set_boolean(env, report, "initialValuePresent", initial_value_present) != napi_ok ||
      set_boolean(env, report, "releasedValueIsNull", released_value_is_null) != napi_ok ||
      set_boolean(env, report, "valueAfterReleaseRefIsNull",
                  value_after_release_ref_is_null) != napi_ok ||
      set_boolean(env, report, "zeroValueIsNull", zero_value_is_null) != napi_ok)
    return NULL;
  return report;
}

static napi_value create_weak_reference(napi_env env, napi_callback_info info) {
  size_t argc = 1;
  napi_value value;
  napi_value result;
  if (napi_get_cb_info(env, info, &argc, &value, NULL, NULL) != napi_ok ||
      argc != 1)
    return NULL;
  if (weak_object_reference != NULL) {
    napi_delete_reference(env, weak_object_reference);
    weak_object_reference = NULL;
  }
  if (napi_create_reference(env, value, 0, &weak_object_reference) != napi_ok ||
      napi_get_undefined(env, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value weak_reference_is_null(napi_env env,
                                         napi_callback_info info) {
  napi_value value = NULL;
  napi_value result;
  (void)info;
  if (weak_object_reference == NULL ||
      napi_get_reference_value(env, weak_object_reference, &value) != napi_ok ||
      napi_get_boolean(env, value == NULL, &result) != napi_ok)
    return NULL;
  return result;
}

static napi_value weak_reference_ref_status(napi_env env,
                                            napi_callback_info info) {
  napi_value result;
  (void)info;
  napi_status status = weak_object_reference == NULL
                           ? napi_invalid_arg
                           : napi_reference_ref(env, weak_object_reference, NULL);
  if (napi_create_int32(env, status, &result) != napi_ok) return NULL;
  return result;
}

static napi_value delete_weak_reference(napi_env env,
                                        napi_callback_info info) {
  napi_value result;
  (void)info;
  if (weak_object_reference == NULL ||
      napi_delete_reference(env, weak_object_reference) != napi_ok ||
      napi_get_undefined(env, &result) != napi_ok)
    return NULL;
  weak_object_reference = NULL;
  return result;
}

NAPI_MODULE_INIT() {
  napi_value function;
  if (napi_create_function(env, "referenceProbe", NAPI_AUTO_LENGTH,
                           reference_probe, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "referenceProbe", function) != napi_ok ||
      napi_create_function(env, "createWeakReference", NAPI_AUTO_LENGTH,
                           create_weak_reference, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "createWeakReference", function) != napi_ok ||
      napi_create_function(env, "weakReferenceIsNull", NAPI_AUTO_LENGTH,
                           weak_reference_is_null, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "weakReferenceIsNull", function) != napi_ok ||
      napi_create_function(env, "weakReferenceRefStatus", NAPI_AUTO_LENGTH,
                           weak_reference_ref_status, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "weakReferenceRefStatus", function) != napi_ok ||
      napi_create_function(env, "deleteWeakReference", NAPI_AUTO_LENGTH,
                           delete_weak_reference, NULL, &function) != napi_ok ||
      napi_set_named_property(env, exports, "deleteWeakReference", function) != napi_ok)
    return NULL;
  return exports;
}
