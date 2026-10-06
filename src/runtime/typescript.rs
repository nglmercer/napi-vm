//! TypeScript is transformed outside the JavaScript engine using Oxc.
use crate::{ModuleLoader, ModuleSource, VmErr};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

pub struct TransformedSource {
    pub javascript: String,
    pub source_map: String,
}
pub fn transform(filename: &Path, source: &str) -> Result<TransformedSource, VmErr> {
    use oxc_allocator::Allocator;
    use oxc_codegen::{Codegen, CodegenOptions};
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;
    use oxc_transformer::{TransformOptions, Transformer};
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(filename).map_err(|e| VmErr::Msg(e.to_string()))?;
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if !parsed.errors.is_empty() {
        return Err(VmErr::Msg(format!(
            "TypeScript syntax error: {:?}",
            parsed.errors[0]
                .clone()
                .with_source_code(source.to_string())
        )));
    }
    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&program);
    if !semantic.errors.is_empty() {
        return Err(VmErr::Msg(format!(
            "TypeScript semantic error: {:?}",
            semantic.errors[0]
                .clone()
                .with_source_code(source.to_string())
        )));
    }
    let options = TransformOptions::default();
    let transformed = Transformer::new(&allocator, filename, &options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if !transformed.errors.is_empty() {
        return Err(VmErr::Msg(format!(
            "TypeScript transform error: {:?}",
            transformed.errors[0]
                .clone()
                .with_source_code(source.to_string())
        )));
    }
    let generated = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(filename.to_path_buf()),
            ..CodegenOptions::default()
        })
        .build(&program);
    Ok(TransformedSource {
        javascript: generated.code,
        source_map: generated
            .map
            .map(|m| m.to_json_string())
            .unwrap_or_default(),
    })
}
/// Wrap any explicit loader; source maps remain available to the embedding host.
pub struct TypeScriptLoader {
    inner: Rc<dyn ModuleLoader>,
    maps: RefCell<HashMap<String, String>>,
}
impl TypeScriptLoader {
    pub fn new(inner: Rc<dyn ModuleLoader>) -> Self {
        Self {
            inner,
            maps: RefCell::new(HashMap::new()),
        }
    }
    pub fn source_map(&self, id: &str) -> Option<String> {
        self.maps.borrow().get(id).cloned()
    }
}
impl ModuleLoader for TypeScriptLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<String, VmErr> {
        self.inner.resolve(specifier, referrer)
    }
    fn load(&self, id: &str) -> Result<ModuleSource, VmErr> {
        let mut loaded = self.inner.load(id)?;
        let path = url::Url::parse(id)
            .ok()
            .map(|u| u.path().to_string())
            .unwrap_or_else(|| id.into());
        let path = Path::new(&path);
        if path
            .extension()
            .is_some_and(|e| matches!(e.to_str(), Some("ts" | "tsx" | "mts")))
        {
            let transformed = transform(path, &loaded.source)?;
            self.maps
                .borrow_mut()
                .insert(id.into(), transformed.source_map);
            loaded.source = transformed.javascript;
        }
        Ok(loaded)
    }
}
