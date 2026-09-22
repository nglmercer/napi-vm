/** Source kinds understood by optional guest compatibility compilers. */
export type GuestSyntax = "js" | "jsx" | "ts" | "tsx";

export type CompilerMode = "none" | "auto" | "swc";

export interface GuestCompilerInput {
  filename: string;
  source: string;
  syntax: GuestSyntax;
}

export interface GuestCompilerOutput {
  code: string;
  map?: string;
}

/** A host-side source transformer. It never executes package code. */
export interface GuestCompiler {
  compile(input: GuestCompilerInput): Promise<GuestCompilerOutput>;
}

export type NpmCompatibilityCategory =
  | "MODULE_RESOLUTION"
  | "UNSUPPORTED_SYNTAX"
  | "SWC_TRANSFORM"
  | "NATIVE_DEPENDENCY";

export class NpmCompatibilityError extends Error {
  readonly category: NpmCompatibilityCategory;

  constructor(category: NpmCompatibilityCategory, message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "NpmCompatibilityError";
    this.category = category;
  }
}
