import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

// `node --check` verifies that app.js parses, but it cannot see that a called
// identifier was never defined (for example a click handler referencing a
// helper that was accidentally omitted). This lightweight static check catches
// that class of bug, which shipped once as an undefined `openSeries`.
const appSource = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "app.js"), "utf8");

// Remove comments and string/template literals so their contents do not look
// like code, while keeping any `${...}` interpolation expressions.
function stripCommentsAndStrings(code) {
  let out = "";
  let i = 0;
  while (i < code.length) {
    const c = code[i];
    const next = code[i + 1];
    if (c === "/" && next === "/") {
      while (i < code.length && code[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && next === "*") {
      i += 2;
      while (i < code.length && !(code[i] === "*" && code[i + 1] === "/")) i++;
      i += 2;
      continue;
    }
    if (c === '"' || c === "'") {
      i++;
      while (i < code.length && code[i] !== c) {
        if (code[i] === "\\") i++;
        i++;
      }
      i++;
      out += '""';
      continue;
    }
    if (c === "`") {
      i++;
      while (i < code.length) {
        if (code[i] === "\\") {
          i += 2;
          continue;
        }
        if (code[i] === "`") {
          i++;
          break;
        }
        if (code[i] === "$" && code[i + 1] === "{") {
          i += 2;
          const start = i;
          let depth = 1;
          while (i < code.length && depth > 0) {
            if (code[i] === "{") depth++;
            else if (code[i] === "}") depth--;
            else if (code[i] === "`") {
              i++;
              while (i < code.length && code[i] !== "`") {
                if (code[i] === "\\") i++;
                i++;
              }
            }
            if (depth > 0) i++;
          }
          out += " " + code.slice(start, i) + " ";
          i++;
          continue;
        }
        i++;
      }
      out += '""';
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

function collectDeclaredNames(code) {
  const declared = new Set();
  let match;
  for (const re of [
    /\bfunction\s+([A-Za-z_$][\w$]*)/g,
    /\b(?:const|let|var)\s+([A-Za-z_$][\w$]*)/g,
    /catch\s*\(\s*([A-Za-z_$][\w$]*)/g,
    /\bfor\s*\(\s*(?:const|let|var)\s+([A-Za-z_$][\w$]*)/g
  ]) {
    while ((match = re.exec(code))) declared.add(match[1]);
  }
  // Destructuring declarations: `const { a, b } = ...` and `const [a, b] = ...`.
  const destructure = /\b(?:const|let|var)\s*[{[]([^}\]]*)[}\]]/g;
  while ((match = destructure.exec(code))) {
    for (const part of match[1].split(",")) {
      const name = part.split(/[:=]/).pop().trim();
      if (/^[A-Za-z_$][\w$]*$/.test(name)) declared.add(name);
    }
  }
  // Imported bindings.
  const imports = /import\s*{([^}]*)}/g;
  while ((match = imports.exec(code))) {
    for (const part of match[1].split(",")) {
      const name = part.split(/\s+as\s+/).pop().trim();
      if (/^[A-Za-z_$][\w$]*$/.test(name)) declared.add(name);
    }
  }
  // Parameters of functions, arrow functions, and single-argument arrows.
  const params = /(?:function\s*[A-Za-z_$\w]*\s*|\)\s*=>\s*|[A-Za-z_$][\w$]*\s*=>\s*)\(([^)]*)\)|(\b[A-Za-z_$][\w$]*)\s*=>/g;
  while ((match = params.exec(code))) {
    if (match[1] != null) {
      for (const part of match[1].split(",")) {
        const name = part.split(/[:=]/)[0].trim();
        if (/^[A-Za-z_$][\w$]*$/.test(name)) declared.add(name);
      }
    } else if (match[2]) {
      declared.add(match[2]);
    }
  }
  return declared;
}

// Language keywords and browser/global APIs that app.js legitimately calls.
const KNOWN_GLOBALS = new Set([
  "async", "await", "yield", "if", "for", "while", "switch", "catch", "return",
  "typeof", "function", "new", "delete", "void", "in", "of", "do", "else",
  "try", "finally", "throw", "case", "super", "this", "class",
  "console", "document", "window", "location", "navigator", "history",
  "URL", "URLSearchParams", "Blob", "File", "FileReader", "fetch",
  "requestAnimationFrame", "cancelAnimationFrame", "setTimeout", "clearTimeout",
  "setInterval", "clearInterval", "queueMicrotask", "AbortController", "Promise",
  "Object", "Array", "String", "Number", "Boolean", "Math", "JSON", "Date",
  "Map", "Set", "WeakMap", "RegExp", "Error", "Symbol", "Proxy", "Reflect",
  "BigInt", "Intl", "Function",
  "parseInt", "parseFloat", "isNaN", "isFinite", "encodeURIComponent",
  "decodeURIComponent", "structuredClone", "IntersectionObserver",
  "ResizeObserver", "MutationObserver", "matchMedia", "localStorage",
  "sessionStorage", "crypto", "performance", "getComputedStyle", "CustomEvent"
]);

test("app.js only calls functions that are defined", () => {
  const code = stripCommentsAndStrings(appSource);
  const declared = collectDeclaredNames(code);
  const undefinedCalls = new Map();
  const calls = /(?<![.\w$"'])([A-Za-z_$][\w$]*)\s*\(/g;
  let match;
  while ((match = calls.exec(code))) {
    const name = match[1];
    if (declared.has(name) || KNOWN_GLOBALS.has(name)) continue;
    undefinedCalls.set(name, (undefinedCalls.get(name) ?? 0) + 1);
  }
  assert.deepEqual(
    [...undefinedCalls.keys()],
    [],
    `app.js calls undefined functions: ${[...undefinedCalls.keys()].join(", ")}. ` +
      "Add real browser globals to KNOWN_GLOBALS, or define the missing function."
  );
});
