import type { GuestCompiler, GuestCompilerInput, GuestCompilerOutput } from "./types";
import { NpmCompatibilityError } from "./types";

/** Leaves source unchanged. This is the default compiler for JavaScript. */
export class IdentityCompiler implements GuestCompiler {
  async compile(input: GuestCompilerInput): Promise<GuestCompilerOutput> {
    return { code: input.source };
  }
}

export interface SwcTransformOptions {
  filename: string;
  sourceMaps: boolean;
  jsc: {
    parser: { syntax: "ecmascript" | "typescript"; jsx?: boolean; tsx?: boolean };
    target: "es2022";
  };
  module: { type: "es6" };
}

export interface SwcTransformResult {
  code: string;
  map?: string | null;
}

/** Injectable shape of `@swc/core`, useful to hosts and tests. */
export interface SwcBackend {
  transform(source: string, options: SwcTransformOptions): Promise<SwcTransformResult>;
}

/**
 * Optional SWC adapter. `@swc/core` is loaded only if compilation is needed,
 * so the identity path has no SWC dependency or startup cost.
 */
export class SwcCompiler implements GuestCompiler {
  private backend?: Promise<SwcBackend>;

  constructor(private readonly suppliedBackend?: SwcBackend) {}

  async compile(input: GuestCompilerInput): Promise<GuestCompilerOutput> {
    try {
      const swc = await this.loadBackend();
      const syntax = input.syntax;
      const parser: SwcTransformOptions["jsc"]["parser"] =
        syntax === "jsx"
          ? { syntax: "ecmascript", jsx: true }
          : syntax === "tsx"
            ? { syntax: "typescript", tsx: true }
            : { syntax: syntax === "ts" ? "typescript" : "ecmascript" };
      const result = await swc.transform(input.source, {
        filename: input.filename,
        sourceMaps: true,
        jsc: {
          parser,
          target: "es2022",
        },
        module: { type: "es6" },
      });
      return { code: result.code, ...(result.map ? { map: result.map } : {}) };
    } catch (cause) {
      if (cause instanceof NpmCompatibilityError) throw cause;
      const detail = cause instanceof Error ? cause.message : String(cause);
      throw new NpmCompatibilityError(
        "SWC_TRANSFORM",
        `SWC could not compile ${input.filename}: ${detail}`,
        { cause },
      );
    }
  }

  private loadBackend(): Promise<SwcBackend> {
    if (this.suppliedBackend) return Promise.resolve(this.suppliedBackend);
    this.backend ??= (async () => {
      try {
        // A variable import keeps @swc/core optional to TypeScript and bundlers.
        const packageName = "@swc/core";
        const loaded = (await import(packageName)) as unknown as {
          default?: SwcBackend;
          transform?: SwcBackend["transform"];
        };
        const backend = loaded.default ?? loaded;
        if (typeof backend.transform !== "function") {
          throw new Error("the installed package does not export transform()");
        }
        return backend as SwcBackend;
      } catch (cause) {
        const detail = cause instanceof Error ? cause.message : String(cause);
        throw new NpmCompatibilityError(
          "SWC_TRANSFORM",
          `SWC mode requires the optional @swc/core package: ${detail}`,
          { cause },
        );
      }
    })();
    return this.backend;
  }
}
