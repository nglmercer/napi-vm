//! Realm-owned module records. Source catalogs may be copied between realms;
//! linked records, namespace identity, failures and CommonJS instances may not.
use super::*;

#[derive(Clone)]
pub(crate) struct ModuleRealm {
    modules: Rc<RefCell<HashMap<String, Module>>>,
    sources: Rc<RefCell<HashMap<String, std::sync::Arc<str>>>>,
    aliases: Rc<RefCell<HashMap<(String, String), String>>>,
    urls: Rc<RefCell<HashMap<String, String>>>,
    evaluating: Rc<RefCell<std::collections::HashSet<String>>>,
    graph: Rc<RefCell<module_link::ModuleGraph>>,
    commonjs_loader: Option<Rc<dyn CommonJsModuleLoader>>,
    loader: Option<Rc<dyn crate::ModuleLoader>>,
    commonjs_cache: Rc<RefCell<HashMap<String, commonjs::CommonJsCacheEntry>>>,
    commonjs_entry: Option<String>,
}
impl ModuleRealm {
    pub(crate) fn of(vm: &Interpreter) -> Self {
        Self {
            modules: vm.modules.clone(),
            sources: vm.module_sources.clone(),
            aliases: vm.module_aliases.clone(),
            urls: vm.module_file_urls.clone(),
            evaluating: vm.evaluating.clone(),
            graph: vm.module_graph.clone(),
            commonjs_loader: vm.commonjs_loader.clone(),
            loader: vm.module_loader.clone(),
            commonjs_cache: vm.commonjs_cache.clone(),
            commonjs_entry: vm.commonjs_entry.clone(),
        }
    }
    pub(crate) fn install(self, vm: &mut Interpreter) {
        vm.modules = self.modules;
        vm.module_sources = self.sources;
        vm.module_aliases = self.aliases;
        vm.module_file_urls = self.urls;
        vm.evaluating = self.evaluating;
        vm.module_graph = self.graph;
        vm.commonjs_loader = self.commonjs_loader;
        vm.module_loader = self.loader;
        vm.commonjs_cache = self.commonjs_cache;
        vm.commonjs_entry = self.commonjs_entry;
    }
    pub(crate) fn fork_sources(parent: &Interpreter, child: &mut Interpreter) {
        *child.module_sources.borrow_mut() = parent.module_sources.borrow().clone();
        *child.module_aliases.borrow_mut() = parent.module_aliases.borrow().clone();
        *child.module_file_urls.borrow_mut() = parent.module_file_urls.borrow().clone();
        child.module_loader = parent.module_loader.clone();
        child.commonjs_loader = parent.commonjs_loader.clone();
        child.commonjs_entry = parent.commonjs_entry.clone();
        child.publish_module_realm();
    }
    pub(crate) fn roots(&self) -> Result<crate::heap::GcRoots, ()> {
        Self::roots_of(&self.modules, &self.graph, &self.commonjs_cache)
    }
    pub(super) fn roots_of(
        modules: &Rc<RefCell<HashMap<String, Module>>>,
        graph: &Rc<RefCell<module_link::ModuleGraph>>,
        cache: &Rc<RefCell<HashMap<String, commonjs::CommonJsCacheEntry>>>,
    ) -> Result<crate::heap::GcRoots, ()> {
        let graph = graph.try_borrow().map_err(|_| ())?;
        let cache = cache.try_borrow().map_err(|_| ())?;
        let mut roots = crate::heap::GcRoots {
            modules: vec![modules.clone()],
            ..Default::default()
        };
        roots.values.extend(graph.trace_values());
        for entry in cache.values() {
            roots.values.push(entry.exports.clone());
            roots.values.extend(entry.module.clone());
        }
        Ok(roots)
    }
}
pub(crate) struct ModuleRealmSwitch(Option<ModuleRealm>);
impl ModuleRealmSwitch {
    pub(crate) fn install(self, vm: &mut Interpreter) {
        if let Some(saved) = self.0 {
            saved.install(vm);
        }
    }
}
impl Interpreter {
    pub(crate) fn publish_module_realm(&self) {
        self.persistent_global.borrow_mut().module_realm = Some(ModuleRealm::of(self));
    }
    pub(crate) fn enter_module_realm(&mut self, owner: &Env) -> ModuleRealmSwitch {
        if Rc::ptr_eq(owner, &self.persistent_global) {
            return ModuleRealmSwitch(None);
        }
        let saved = ModuleRealm::of(self);
        let realm = owner.borrow().module_realm.clone();
        if let Some(realm) = realm {
            realm.install(self);
        }
        ModuleRealmSwitch(Some(saved))
    }
}
