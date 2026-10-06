//! Cached document service for editor and future LSP adapters.
//!
//! This is deliberately transport-neutral. A browser adapter can call it
//! directly through WASM, while an LSP adapter can map `didOpen`, `didChange`,
//! and request messages onto these methods without duplicating analysis.

use std::collections::HashMap;

use super::{
    AnalysisContext, Completion, Diagnostic, Document, GlobalInfo, HostFunctionInfo, HoverInfo,
    ModuleInfo, Symbol, Type, complete, diagnose, metadata, symbols,
};

#[derive(Debug, Default)]
pub struct LanguageService {
    documents: HashMap<String, Document>,
    modules: HashMap<String, ModuleInfo>,
    module_sources: HashMap<String, String>,
    host_functions: HashMap<String, HostFunctionInfo>,
    manifest_host_functions: HashMap<String, HostFunctionInfo>,
    runtime_handlers: HashMap<String, Type>,
    runtime_values: HashMap<String, String>,
    manifest_globals: HashMap<String, GlobalInfo>,
    runtime_globals: HashMap<String, GlobalInfo>,
}

impl LanguageService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open or replace a document, equivalent to LSP `textDocument/didOpen`.
    pub fn open(&mut self, uri: impl Into<String>, source: &str) {
        let uri = uri.into();
        self.documents.insert(uri, self.parse(source));
    }

    /// Replace a document snapshot, equivalent to LSP `textDocument/didChange`.
    pub fn update(&mut self, uri: &str, source: &str) -> bool {
        if !self.documents.contains_key(uri) {
            return false;
        }
        self.documents.insert(uri.to_string(), self.parse(source));
        true
    }

    pub fn register_module(&mut self, name: impl Into<String>, source: &str) {
        let name = name.into();
        let document = self.parse_with_globals(source);
        self.modules.insert(
            name.clone(),
            ModuleInfo {
                name: name.clone(),
                exports: document.export_names(),
            },
        );
        self.module_sources.insert(name, source.to_string());
        self.rebuild_documents();
    }

    /// Register a host function exposed by the live runtime.
    pub fn register_host_function(&mut self, function: HostFunctionInfo) {
        self.host_functions.insert(function.name.clone(), function);
        self.rebuild_documents();
    }

    /// Register a host function declared by the static project manifest. These
    /// sit below everything the live runtime publishes, so a stale manifest
    /// entry never masks a function the running program actually exposed.
    pub fn register_manifest_host_function(&mut self, function: HostFunctionInfo) {
        self.manifest_host_functions
            .insert(function.name.clone(), function);
        self.rebuild_documents();
    }

    /// Register a generic global published by the live runtime. Re-registering
    /// a name replaces the previous declaration for that runtime generation.
    pub fn register_runtime_global(&mut self, global: GlobalInfo) {
        self.runtime_globals.insert(global.name.clone(), global);
        self.rebuild_documents();
    }

    /// Register a generic global from the static project manifest.
    pub fn register_manifest_global(&mut self, global: GlobalInfo) {
        self.manifest_globals.insert(global.name.clone(), global);
        self.rebuild_documents();
    }

    /// Convenience alias for embedders that supply live metadata directly.
    pub fn register_global(&mut self, global: GlobalInfo) {
        self.register_runtime_global(global);
    }

    pub fn clear_runtime_globals(&mut self) {
        if !self.runtime_globals.is_empty() {
            self.runtime_globals.clear();
            self.rebuild_documents();
        }
    }

    /// Register a JSON shape observed for a VM event handler. The shape is
    /// intentionally metadata-only: it is never executed and only improves
    /// completion/hover for the handler's first parameter.
    pub fn register_runtime_shape(
        &mut self,
        name: impl Into<String>,
        shape_json: &str,
        last_value_json: Option<&str>,
    ) -> bool {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(shape_json) else {
            return false;
        };
        let Ok(shape) = metadata::parse_shape(&value) else {
            return false;
        };
        let name = name.into();
        self.runtime_handlers
            .insert(name.clone(), Type::from_shape(&shape));
        if let Some(value_json) = last_value_json
            && let Ok(value) = serde_json::from_str::<serde_json::Value>(value_json)
            && let Ok(pretty) = serde_json::to_string_pretty(&value)
        {
            self.runtime_values.insert(name, pretty);
        }
        self.rebuild_documents();
        true
    }

    /// Close and release a document, equivalent to LSP `didClose`.
    pub fn close(&mut self, uri: &str) -> bool {
        self.documents.remove(uri).is_some()
    }

    pub fn source(&self, uri: &str) -> Option<&str> {
        self.documents.get(uri).map(Document::source)
    }

    pub fn hover(&self, uri: &str, offset: usize) -> Option<HoverInfo> {
        self.documents.get(uri)?.hover(offset)
    }

    pub fn complete(
        &self,
        uri: &str,
        offset: usize,
        context: &AnalysisContext,
    ) -> Option<Vec<Completion>> {
        Some(complete(self.documents.get(uri)?.source(), offset, context))
    }

    pub fn diagnostics(&self, uri: &str) -> Option<Vec<Diagnostic>> {
        Some(diagnose(self.documents.get(uri)?.source()))
    }

    pub fn symbols(&self, uri: &str) -> Option<Vec<Symbol>> {
        Some(symbols(self.documents.get(uri)?.source()))
    }

    pub fn context(&self) -> AnalysisContext {
        AnalysisContext {
            exposed_functions: self.hosts(),
            manifest_functions: self.manifest_hosts(),
            modules: self.modules.values().cloned().collect(),
            runtime_handlers: self.runtime_handlers.clone(),
            runtime_globals: self.runtime_globals.clone(),
            manifest_globals: self.manifest_globals.clone(),
        }
    }

    fn hosts(&self) -> Vec<HostFunctionInfo> {
        let mut hosts: Vec<_> = self.host_functions.values().cloned().collect();
        hosts.sort_by(|a, b| a.name.cmp(&b.name));
        hosts
    }

    /// Manifest host functions that the live runtime has not superseded.
    fn manifest_hosts(&self) -> Vec<HostFunctionInfo> {
        let mut hosts: Vec<_> = self
            .manifest_host_functions
            .values()
            .filter(|function| !self.host_functions.contains_key(&function.name))
            .cloned()
            .collect();
        hosts.sort_by(|a, b| a.name.cmp(&b.name));
        hosts
    }

    fn parse(&self, source: &str) -> Document {
        let context = self.context();
        let globals = context.resolved_globals();
        Document::parse_with_context_and_runtime_and_globals(
            source,
            &self.module_sources,
            &context.host_functions(),
            &self.runtime_handlers,
            &self.runtime_values,
            &globals,
        )
    }

    fn parse_with_globals(&self, source: &str) -> Document {
        self.parse(source)
    }

    fn rebuild_documents(&mut self) {
        let sources: Vec<_> = self
            .documents
            .iter()
            .map(|(uri, document)| (uri.clone(), document.source().to_string()))
            .collect();
        for (uri, source) in sources {
            self.documents.insert(uri, self.parse(&source));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{CompletionKind, HostFunctionParameter};
    use crate::lang::{ParameterInfo, PropertyInfo, Shape};
    use std::collections::BTreeMap;

    fn global(name: &str, properties: Vec<(&str, Shape, Option<&str>)>) -> GlobalInfo {
        GlobalInfo {
            name: name.into(),
            shape: Shape::Object(
                properties
                    .into_iter()
                    .map(|(name, shape, documentation)| {
                        (
                            name.into(),
                            PropertyInfo {
                                shape,
                                documentation: documentation.map(str::to_string),
                            },
                        )
                    })
                    .collect(),
            ),
            documentation: None,
        }
    }

    #[test]
    fn document_lifecycle_rebuilds_analysis() {
        let mut service = LanguageService::new();
        service.open("file:///async.js", "const total = 1; total;");
        let offset = service
            .source("file:///async.js")
            .unwrap()
            .rfind("total")
            .unwrap()
            + 1;
        assert_eq!(
            service.hover("file:///async.js", offset).unwrap().detail,
            "const total: number"
        );
        assert!(service.update("file:///async.js", "const total = \"ready\"; total;"));
        let offset = service
            .source("file:///async.js")
            .unwrap()
            .rfind("total")
            .unwrap()
            + 1;
        assert_eq!(
            service.hover("file:///async.js", offset).unwrap().detail,
            "const total: string"
        );
        assert!(service.close("file:///async.js"));
        assert!(service.hover("file:///async.js", 0).is_none());
    }

    #[test]
    fn host_metadata_rebuilds_open_documents() {
        let mut service = LanguageService::new();
        service.open("file:///main.js", "alert(\"hello\");");
        service.register_host_function(HostFunctionInfo {
            name: "alert".into(),
            params: vec![HostFunctionParameter {
                name: "message".into(),
                type_name: "string".into(),
            }],
            return_type: "void".into(),
            documentation: Some("Displays a message.".into()),
            async_fn: false,
        });

        let hover = service.hover("file:///main.js", 2).unwrap();
        assert_eq!(hover.detail, "(function) alert: (message: string) => void");
        assert_eq!(hover.documentation.as_deref(), Some("Displays a message."));
        let completions = service
            .complete("file:///main.js", 2, &service.context())
            .unwrap();
        assert!(completions.iter().any(|item| item.label == "alert"));
    }

    #[test]
    fn registered_modules_feed_import_inference() {
        let mut service = LanguageService::new();
        service.register_module(
            "math",
            "export function double(value) { return value * 2; }",
        );
        service.open(
            "file:///main.js",
            "import { double } from \"math\";\ndouble;",
        );

        let source = service.source("file:///main.js").unwrap();
        let offset = source.rfind("double").unwrap() + 1;
        let hover = service.hover("file:///main.js", offset).unwrap();
        assert!(hover.detail.contains("double"));
        assert!(hover.detail.contains("=> number"));
    }

    #[test]
    fn runtime_json_shape_feeds_handler_completion() {
        let mut service = LanguageService::new();
        service.register_runtime_shape(
            "handleChat",
            r#"{
                "kind":"object",
                "properties":{
                    "platform":{"kind":"string"},
                    "data":{"kind":"object","properties":{
                        "nickname":{"kind":"string"},
                        "comment":{"kind":"string"}
                    }}
                }
            }"#,
            Some(r#"{"platform":"tiktok","eventName":"chat","data":{"nickname":"Ada","comment":"hello"}}"#),
        );
        let source = "function handleChat(event) { event.data.";
        service.open("file:///chat.js", source);

        let completions = service
            .complete("file:///chat.js", source.len(), &service.context())
            .unwrap();
        let labels: Vec<_> = completions.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"nickname"));
        assert!(labels.contains(&"comment"));

        let event_offset = source.find("event").unwrap() + 2;
        let hover = service.hover("file:///chat.js", event_offset).unwrap();
        assert!(hover.detail.contains("parameter"));
        assert!(hover.detail.contains("platform"));
        assert!(hover.documentation.as_deref().unwrap().contains("Ada"));
    }

    #[test]
    fn generic_globals_support_nested_completion_hover_signatures_and_docs() {
        let mut service = LanguageService::new();
        service.register_runtime_global(global(
            "customApi",
            vec![(
                "user",
                Shape::Object(BTreeMap::from([
                    (
                        "fetchUser".into(),
                        PropertyInfo {
                            shape: Shape::Function {
                                params: vec![ParameterInfo {
                                    name: "id".into(),
                                    shape: Shape::String,
                                }],
                                returns: Box::new(Shape::Object(BTreeMap::from([(
                                    "name".into(),
                                    PropertyInfo {
                                        shape: Shape::String,
                                        documentation: None,
                                    },
                                )]))),
                                async_fn: false,
                            },
                            documentation: Some("Fetches one user.".into()),
                        },
                    ),
                    (
                        "loadAsync".into(),
                        PropertyInfo {
                            shape: Shape::Function {
                                params: vec![],
                                returns: Box::new(Shape::String),
                                async_fn: true,
                            },
                            documentation: None,
                        },
                    ),
                    (
                        "names".into(),
                        PropertyInfo {
                            shape: Shape::Array(Box::new(Shape::String)),
                            documentation: None,
                        },
                    ),
                ])),
                None,
            )],
        ));
        service.open(
            "file:///custom.js",
            "customApi.user.fetchUser; customApi.user.; customApi.user.loadAsync; customApi.user.names;",
        );

        let source = service.source("file:///custom.js").unwrap();
        let nested_offset = source.find("customApi.user.").unwrap() + "customApi.user.".len();
        let nested = service
            .complete("file:///custom.js", nested_offset, &service.context())
            .unwrap();
        assert!(nested.iter().any(|item| item.label == "fetchUser"));
        let fetch = nested
            .iter()
            .find(|item| item.label == "fetchUser")
            .unwrap();
        assert_eq!(
            fetch.detail.as_deref(),
            Some("(id: string) => { name: string }")
        );

        let fetch_offset = source.rfind("fetchUser").unwrap() + 2;
        let fetch_hover = service.hover("file:///custom.js", fetch_offset).unwrap();
        assert_eq!(
            fetch_hover.detail,
            "(property) fetchUser: (id: string) => { name: string }"
        );
        assert_eq!(
            fetch_hover.documentation.as_deref(),
            Some("Fetches one user.")
        );

        let async_offset = source.rfind("loadAsync").unwrap() + 2;
        assert!(
            service
                .hover("file:///custom.js", async_offset)
                .unwrap()
                .detail
                .contains("() => Promise<string>")
        );
        let names_offset = source.rfind("names").unwrap() + 2;
        assert_eq!(
            service
                .hover("file:///custom.js", names_offset)
                .unwrap()
                .detail,
            "(property) names: string[]"
        );
    }

    fn host_function(name: &str, returns: &str) -> HostFunctionInfo {
        HostFunctionInfo {
            name: name.into(),
            params: vec![HostFunctionParameter {
                name: "id".into(),
                type_name: "number".into(),
            }],
            return_type: returns.into(),
            documentation: Some(format!("{name} documentation")),
            async_fn: false,
        }
    }

    /// A stale static manifest must never describe a name the running program
    /// is currently exposing. Before this, `resolved_globals()` merged only the
    /// generic layers and host functions were consulted afterwards, so the
    /// manifest declaration won and the editor showed the wrong type.
    #[test]
    fn live_host_function_outranks_a_manifest_generic_global() {
        let mut service = LanguageService::new();
        service.register_manifest_global(GlobalInfo {
            name: "api".into(),
            shape: Shape::String,
            documentation: Some("from the static manifest".into()),
        });
        service.register_host_function(host_function("api", "number"));
        service.open("file:///collision.js", "api");

        let completions = service
            .complete("file:///collision.js", 3, &service.context())
            .unwrap();
        let api = completions
            .iter()
            .find(|item| item.label == "api")
            .expect("api offered");
        assert_eq!(api.kind, CompletionKind::ExposedFn);
        assert_eq!(api.detail.as_deref(), Some("(id: number) => number"));

        let hover = service.hover("file:///collision.js", 0).unwrap();
        assert_eq!(hover.detail, "(function) api: (id: number) => number");
        assert_eq!(hover.documentation.as_deref(), Some("api documentation"));
    }

    /// ...but an explicit generic global from the same live runtime still wins
    /// over that runtime's legacy host function.
    #[test]
    fn runtime_generic_global_outranks_a_runtime_host_function() {
        let mut service = LanguageService::new();
        service.register_host_function(host_function("api", "number"));
        service.register_runtime_global(global("api", vec![("version", Shape::String, None)]));
        service.open("file:///generic.js", "api.");

        let completions = service
            .complete("file:///generic.js", 4, &service.context())
            .unwrap();
        assert!(completions.iter().any(|item| item.label == "version"));
    }

    /// A manifest host function is a fallback, not an override: the live
    /// runtime's declaration of the same name replaces it entirely.
    #[test]
    fn runtime_host_function_replaces_the_manifest_declaration() {
        let mut service = LanguageService::new();
        service.register_manifest_host_function(host_function("api", "string"));
        service.register_manifest_host_function(host_function("legacy", "string"));
        service.register_host_function(host_function("api", "number"));
        service.open("file:///hosts.js", "");

        let context = service.context();
        let hosts = context.host_functions();
        let api = hosts.iter().find(|f| f.name == "api").expect("api present");
        assert_eq!(api.return_type, "number");
        assert_eq!(hosts.iter().filter(|f| f.name == "api").count(), 1);
        // A manifest-only function the runtime never mentioned still survives.
        assert!(hosts.iter().any(|f| f.name == "legacy"));
    }

    #[test]
    fn generic_global_precedence_is_local_then_runtime_then_manifest() {
        let mut service = LanguageService::new();
        service.register_manifest_global(global("api", vec![("version", Shape::String, None)]));
        service.register_runtime_global(global("api", vec![("version", Shape::Number, None)]));
        service.register_runtime_global(global("Math", vec![("runtimeOnly", Shape::Number, None)]));
        service.register_runtime_global(global("ipc", vec![("runtimeOnly", Shape::Number, None)]));
        service.open(
            "file:///precedence.js",
            "const Math = { custom: 1 }; Math.; const ipc = { localOnly: true }; ipc.; window.ipc.; api.version;",
        );

        let source = service.source("file:///precedence.js").unwrap();
        let math_offset = source.find("Math.").unwrap() + "Math.".len();
        let math = service
            .complete("file:///precedence.js", math_offset, &service.context())
            .unwrap();
        assert!(math.iter().any(|item| item.label == "custom"));
        assert!(!math.iter().any(|item| item.label == "runtimeOnly"));

        let ipc_offset = source.find("ipc.").unwrap() + "ipc.".len();
        let ipc = service
            .complete("file:///precedence.js", ipc_offset, &service.context())
            .unwrap();
        assert!(ipc.iter().any(|item| item.label == "localOnly"));

        let alias_offset = source.find("window.ipc.").unwrap() + "window.ipc.".len();
        let alias = service
            .complete("file:///precedence.js", alias_offset, &service.context())
            .unwrap();
        assert!(alias.iter().any(|item| item.label == "runtimeOnly"));
        assert!(!alias.iter().any(|item| item.label == "localOnly"));

        let version_offset = source.rfind("version").unwrap() + 2;
        assert!(
            service
                .hover("file:///precedence.js", version_offset)
                .unwrap()
                .detail
                .contains("number")
        );
        service.clear_runtime_globals();
        let version_offset = service
            .source("file:///precedence.js")
            .unwrap()
            .rfind("version")
            .unwrap()
            + 2;
        assert!(
            service
                .hover("file:///precedence.js", version_offset)
                .unwrap()
                .detail
                .contains("string")
        );
    }
}
