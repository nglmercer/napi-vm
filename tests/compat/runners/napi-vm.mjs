import { Vm } from "../../../index.js";
import { GuestPackageLoader } from "../../../plugins/npm/index.ts";
import { nodePlatform } from "../../../plugins/node.ts";

const source = await Bun.file(process.argv[2]).text();
const vm = new Vm();
const loader = new GuestPackageLoader(vm, { platform: nodePlatform(), compilerMode: "none" });
await loader.loadPackage("valibot");
const validation = vm.validateModule(source);
if (!validation.valid) {
  const diagnostics = validation.diagnostics
    .map(({ line, column, message, kind }) => `${kind} at ${line}:${column}: ${message}`)
    .join("\n");
  throw new SyntaxError(`${process.argv[2]}\n${diagnostics}`);
}
vm.defineModule(process.argv[2], source);
vm.run(source);
vm.dispose();
