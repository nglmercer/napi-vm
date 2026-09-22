export interface StaticImport {
  specifier: string;
  start: number;
  end: number;
}

export interface ScannedModuleSource {
  imports: StaticImport[];
  hasDefaultExport: boolean;
  hasNonLiteralDynamicImport: boolean;
  hasCommonJs: boolean;
}

interface Token {
  kind: "identifier" | "string" | "punctuation";
  value: string;
  start: number;
  end: number;
  depth: number;
}

function decodeString(raw: string): string {
  let result = "";
  for (let i = 0; i < raw.length; i++) {
    const character = raw[i];
    if (character !== "\\" || i + 1 >= raw.length) {
      result += character;
      continue;
    }
    const escaped = raw[++i];
    if (escaped === "n") result += "\n";
    else if (escaped === "r") result += "\r";
    else if (escaped === "t") result += "\t";
    else if (escaped === "b") result += "\b";
    else if (escaped === "f") result += "\f";
    else if (escaped === "v") result += "\v";
    else if (escaped === "0") result += "\0";
    else result += escaped;
  }
  return result;
}

function tokenize(source: string): Token[] {
  const tokens: Token[] = [];
  let index = 0;
  let depth = 0;
  while (index < source.length) {
    const character = source[index];
    if (/\s/.test(character)) {
      index++;
      continue;
    }
    if (character === "/" && source[index + 1] === "/") {
      index += 2;
      while (index < source.length && source[index] !== "\n") index++;
      continue;
    }
    if (character === "/" && source[index + 1] === "*") {
      index += 2;
      while (index < source.length && !(source[index] === "*" && source[index + 1] === "/")) index++;
      index = Math.min(source.length, index + 2);
      continue;
    }
    if (character === "'" || character === '"') {
      const start = index++;
      let value = "";
      while (index < source.length && source[index] !== character) {
        if (source[index] === "\\" && index + 1 < source.length) {
          value += source.slice(index, index + 2);
          index += 2;
        } else {
          value += source[index++];
        }
      }
      if (index < source.length) index++;
      tokens.push({ kind: "string", value: decodeString(value), start, end: index, depth });
      continue;
    }
    // Import declarations cannot occur inside a template literal. Skipping a
    // template as one token also prevents text inside it looking like syntax.
    if (character === "`") {
      index++;
      while (index < source.length) {
        if (source[index] === "\\") index += 2;
        else if (source[index++] === "`") break;
      }
      continue;
    }
    if (/[A-Za-z_$]/.test(character)) {
      const start = index++;
      while (index < source.length && /[A-Za-z0-9_$]/.test(source[index])) index++;
      tokens.push({ kind: "identifier", value: source.slice(start, index), start, end: index, depth });
      continue;
    }
    if (character === "}" || character === ")" || character === "]") depth = Math.max(0, depth - 1);
    tokens.push({ kind: "punctuation", value: character, start: index, end: index + 1, depth });
    if (character === "{" || character === "(" || character === "[") depth++;
    index++;
  }
  return tokens;
}

function findFromString(tokens: Token[], start: number): Token | undefined {
  for (let i = start; i < tokens.length; i++) {
    const token = tokens[i];
    if (token.depth === 0 && token.value === ";") return undefined;
    if (token.depth === 0 && token.kind === "identifier" && ["import", "export", "const", "let", "var", "function", "class"].includes(token.value)) {
      return undefined;
    }
    if (token.kind === "identifier" && token.value === "from" && tokens[i + 1]?.kind === "string") {
      return tokens[i + 1];
    }
  }
  return undefined;
}

/** Extract static ESM specifiers while ignoring comments, strings and dynamic imports. */
export function scanModuleSource(source: string): ScannedModuleSource {
  const tokens = tokenize(source);
  const imports: StaticImport[] = [];
  let hasDefaultExport = false;
  let hasNonLiteralDynamicImport = false;
  let hasCommonJs = false;
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i];
    if (token.kind !== "identifier") continue;
    if (
      (token.value === "require" && tokens[i + 1]?.value === "(") ||
      (token.value === "module" && tokens[i + 1]?.value === "." && tokens[i + 2]?.value === "exports") ||
      (token.value === "exports" && [".", "["].includes(tokens[i + 1]?.value ?? ""))
    ) {
      hasCommonJs = true;
    }
    if (token.value === "import") {
      const next = tokens[i + 1];
      if (!next || next.value === ".") continue;
      if (next.value === "(") {
        const specifier = tokens[i + 2];
        if (specifier?.kind === "string") {
          imports.push({ specifier: specifier.value, start: specifier.start, end: specifier.end });
        } else {
          hasNonLiteralDynamicImport = true;
        }
        continue;
      }
      if (token.depth !== 0) continue;
      const specifier = next.kind === "string" ? next : findFromString(tokens, i + 2);
      if (specifier) imports.push({ specifier: specifier.value, start: specifier.start, end: specifier.end });
    } else if (token.value === "export" && token.depth === 0) {
      const next = tokens[i + 1];
      if (!next) continue;
      if (next.value === "default") hasDefaultExport = true;
      if (next.value === "{") {
        let specifierParts: Token[] = [];
        const checkDefault = () => {
          const identifiers = specifierParts.filter((part) => part.kind === "identifier");
          if (identifiers.length === 0) return;
          if (identifiers[0].value === "default" && identifiers.length === 1) {
            hasDefaultExport = true;
          } else if (identifiers.length >= 3 && identifiers[1].value === "as" && identifiers[2].value === "default") {
            hasDefaultExport = true;
          }
          specifierParts = [];
        };
        for (let j = i + 2; j < tokens.length; j++) {
          const part = tokens[j];
          if (part.value === "}" && part.depth === 0) {
            checkDefault();
            if (tokens[j + 1]?.value === "from" && tokens[j + 2]?.kind === "string") {
              const specifier = tokens[j + 2];
              imports.push({ specifier: specifier.value, start: specifier.start, end: specifier.end });
            }
            break;
          }
          if (part.depth === 1) {
            if (part.value === ",") checkDefault();
            else specifierParts.push(part);
          }
        }
      } else if (next.value === "*") {
        if (tokens[i + 2]?.value === "as" && tokens[i + 3]?.value === "default") hasDefaultExport = true;
        const from = findFromString(tokens, i + 2);
        if (from) imports.push({ specifier: from.value, start: from.start, end: from.end });
      }
    }
  }
  return { imports, hasDefaultExport, hasNonLiteralDynamicImport, hasCommonJs };
}

/** Replace import/export specifier string literals, from right to left. */
export function rewriteModuleSpecifiers(
  source: string,
  imports: readonly StaticImport[],
  targets: readonly string[],
): string {
  const replacements = imports
    .map((item, index) => ({ ...item, target: targets[index] }))
    .sort((a, b) => b.start - a.start);
  let result = source;
  for (const item of replacements) {
    result = `${result.slice(0, item.start)}${JSON.stringify(item.target)}${result.slice(item.end)}`;
  }
  return result;
}
