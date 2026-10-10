import { test } from "bun:test";
import { Vm } from "../index.js";

export function runtimeFixture(options) {
  const probe = new Vm();
  let available = false;
  try {
    probe.enableRuntime(options);
    available = true;
  } catch (error) {
    if (!/Cargo feature|^Web globals require runtime-web$/.test(error.message)) throw error;
  } finally {
    probe.dispose();
  }
  return {
    test: available ? test : test.skip,
    run(source) {
      const vm = new Vm();
      try {
        vm.enableRuntime(options);
        return vm.run(source);
      } finally {
        vm.dispose();
      }
    },
  };
}
