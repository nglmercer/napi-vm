//! Explicit Node compatibility modules. Unsupported modules fail resolution.
use crate::{Interpreter, VmErr};
pub(super) const MODULES: &[(&str, &str, &str)] = &[
    (
        "node:path",
        "__napiVmNodePath",
        "export const normalize=__napiVmNodePath.normalize,join=__napiVmNodePath.join,resolve=__napiVmNodePath.resolve,dirname=__napiVmNodePath.dirname,basename=__napiVmNodePath.basename,extname=__napiVmNodePath.extname,isAbsolute=__napiVmNodePath.isAbsolute,relative=__napiVmNodePath.relative,parse=__napiVmNodePath.parse,format=__napiVmNodePath.format,sep='/',delimiter=':',posix=__napiVmNodePath;export default __napiVmNodePath;",
    ),
    (
        "node:events",
        "__napiVmNodeEvents",
        "export const EventEmitter=__napiVmNodeEvents.EventEmitter,once=__napiVmNodeEvents.once;export default EventEmitter;",
    ),
    (
        "node:assert",
        "__napiVmNodeAssert",
        "export const ok=__napiVmNodeAssert.ok,equal=__napiVmNodeAssert.equal,strictEqual=__napiVmNodeAssert.strictEqual,notStrictEqual=__napiVmNodeAssert.notStrictEqual,deepStrictEqual=__napiVmNodeAssert.deepStrictEqual,throws=__napiVmNodeAssert.throws,rejects=__napiVmNodeAssert.rejects,fail=__napiVmNodeAssert.fail;export default __napiVmNodeAssert;",
    ),
    (
        "node:util",
        "__napiVmNodeUtil",
        "export const format=__napiVmNodeUtil.format,inspect=__napiVmNodeUtil.inspect,promisify=__napiVmNodeUtil.promisify,callbackify=__napiVmNodeUtil.callbackify,inherits=__napiVmNodeUtil.inherits;export default __napiVmNodeUtil;",
    ),
    (
        "node:buffer",
        "__napiVmNodeBuffer",
        "export const Buffer=__napiVmNodeBuffer.Buffer;export default __napiVmNodeBuffer;",
    ),
];
pub fn is_builtin(name: &str) -> bool {
    MODULES
        .iter()
        .any(|(id, _, _)| *id == name || id.strip_prefix("node:") == Some(name))
}
pub(super) fn commonjs_source(name: &str) -> Option<String> {
    MODULES
        .iter()
        .find(|(id, _, _)| *id == name || id.strip_prefix("node:") == Some(name))
        .map(|(_, global, _)| format!("module.exports={global};"))
}
pub(super) fn install(interpreter: &mut Interpreter) -> Result<(), VmErr> {
    interpreter.eval_source(include_str!("node.js"))?;
    for (id, _, source) in MODULES {
        interpreter.define_module(id, (*source).into());
    }
    Ok(())
}
