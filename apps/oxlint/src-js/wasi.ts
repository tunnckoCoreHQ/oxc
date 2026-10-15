/*
 * Host side of the oxlint CLI built for `wasm32-wasip1`: the `oxlint_host` imports.
 *
 * They stand in for the JS callbacks which the NAPI build gets from `cli.ts`.
 * JS plugins run on the same runtime as with NAPI. Only the file source differs, as there is no raw transfer:
 * the AST comes from oxc-parser's wasm build, and tokens and comments come from Rust, in their raw transfer layout.
 *
 * A call passes strings and byte arrays as pointer and length, and returns a status (`0` for success).
 * The host keeps the response bytes until `response_read` copies them into wasm memory.
 * With the `jspi` option, callbacks which load configs or plugins can be asynchronous (JSPI), and the host calls
 * the command's `_start` through `WebAssembly.promising`.
 */

// Patch `WeakMap` before any plugins are loaded
import "./plugins/weak_map.ts";

import { comments, initComments } from "./plugins/comments.ts";
import { setupRuleConfigs } from "./plugins/config.ts";
import { lintFile, NO_BUFFER_ID } from "./plugins/lint.ts";
import { registerPlugin } from "./plugins/load.ts";
import { getNodeLoc } from "./plugins/location.ts";
import { compileSchema } from "./plugins/options.ts";
import { setupSourceFromFileSource } from "./plugins/source_code.ts";
import { initTokens, tokens } from "./plugins/tokens.ts";
import { getErrorMessage } from "./utils/utils.ts";
import { createWorkspace, destroyWorkspace } from "./workspace/index.ts";

import type { Program } from "./generated/types.d.ts";
import type { Plugin } from "./plugins/load.ts";

export interface OxlintHostOptions {
  /**
   * `parseSync` of oxc-parser's wasm build (`@oxc-parser/binding-wasm32-wasip1`),
   * which parses the files that JS plugins lint.
   */
  parseSync: (filename: string, sourceText: string, options: object) => { program: string };
  /**
   * Import a JS plugin module from its `file://` URL. `import(url)` in Node.js.
   */
  importPlugin: (url: string) => Promise<{ default: unknown }> | { default: unknown };
  /**
   * Evaluate JS/TS config files (`oxlint.config.ts`).
   * Same contract as the `loadJsConfigs` callback of the NAPI build: JSON of
   * `{ Success: { path, config }[] }`, `{ Failures: { path, error }[] }`, or `{ Error: string }`.
   */
  loadJsConfigs: (paths: string[]) => Promise<string> | string;
  /**
   * Run `tsgolint` with `args` to completion, in `cwd`, over the same filesystem.
   * Without it, type-aware linting fails.
   */
  runTsgolint?: (
    cwd: string,
    args: string[],
    stdin: Uint8Array,
  ) => { exitCode: number; stdout: Uint8Array };
  /**
   * `true` if the host calls the command's `_start` through `WebAssembly.promising` (JSPI).
   * Then `importPlugin` and `loadJsConfigs` may return promises.
   */
  jspi?: boolean;
}

// JSPI (WebAssembly JavaScript Promise Integration), not yet in TypeScript's types
const { Suspending } = (
  globalThis as unknown as {
    WebAssembly: {
      Suspending?: new (fn: (...args: number[]) => Promise<number>) => Function;
    };
  }
).WebAssembly;

const textEncoder = new TextEncoder(),
  textDecoder = new TextDecoder();

/**
 * Create the `oxlint_host` imports.
 * @param options - Host callbacks
 * @param getMemory - Get memory of the wasm instance
 * @returns Imports object, to merge into the instance's imports
 */
export function createOxlintHostImports(
  options: OxlintHostOptions,
  getMemory: () => { buffer: ArrayBuffer },
): { oxlint_host: Record<string, Function> } {
  let response: Uint8Array = new Uint8Array();

  const respond = (status: number, body: string | Uint8Array): number => {
    response = typeof body === "string" ? textEncoder.encode(body) : body;
    return status;
  };
  const bytes = (ptr: number, len: number) => new Uint8Array(getMemory().buffer, ptr, len);
  const str = (ptr: number, len: number) => textDecoder.decode(bytes(ptr, len));

  // Wrap a function which may return a promise. With JSPI, wasm waits for the promise.
  const suspending = (fn: (...args: number[]) => number | Promise<number>): Function => {
    if (options.jspi) return new Suspending!(async (...args) => fn(...args));
    return (...args: number[]) => {
      const status = fn(...args);
      if (typeof status === "number") return status;
      throw new Error("A host callback returned a promise, which needs the `jspi` option");
    };
  };

  return {
    oxlint_host: {
      load_js_configs: suspending((pathsPtr, pathsLen) => {
        const paths = JSON.parse(str(pathsPtr, pathsLen)) as string[];
        try {
          const json = options.loadJsConfigs(paths);
          if (typeof json === "string") return respond(0, json);
          return json.then(
            (json) => respond(0, json),
            (err) => respond(1, getErrorMessage(err)),
          );
        } catch (err) {
          return respond(1, getErrorMessage(err));
        }
      }),

      load_plugin: suspending((requestPtr, requestLen) => {
        const { url, pluginName, pluginNameIsAlias } = JSON.parse(str(requestPtr, requestLen));
        const register = (module: { default: unknown }) => {
          const res = registerPlugin(module.default as Plugin, pluginName, pluginNameIsAlias, null);
          return respond(0, JSON.stringify({ Success: res }));
        };
        const fail = (err: unknown) =>
          respond(0, JSON.stringify({ Failure: getErrorMessage(err) }));
        try {
          const module = options.importPlugin(url);
          if (!(module instanceof Promise)) return register(module);
          return module.then(register).catch(fail);
        } catch (err) {
          return fail(err);
        }
      }),

      setup_rule_configs(optionsPtr: number, optionsLen: number): number {
        const err = setupRuleConfigs(str(optionsPtr, optionsLen));
        return err === null ? respond(0, "") : respond(1, err);
      },

      lint_file(
        requestPtr: number,
        requestLen: number,
        settingsPtr: number,
        settingsLen: number,
        globalsPtr: number,
        globalsLen: number,
        sourcePtr: number,
        sourceLen: number,
        tokensPtr: number,
        tokensLen: number,
        commentsPtr: number,
        commentsLen: number,
      ): number {
        const { filePath, ruleIds, optionsIds, collectTimings, lang, sourceType, hasBom } =
          JSON.parse(str(requestPtr, requestLen));
        const sourceText = str(sourcePtr, sourceLen);

        setupSourceFromFileSource(
          {
            sourceText,
            isTs: lang === "ts" || lang === "tsx" || lang === "dts",
            isJsx: lang === "jsx" || lang === "tsx",
            // Copies, because JS side writes "deserialized" flags into them, and wasm memory can grow
            tokens: bytes(tokensPtr, tokensLen).slice(),
            comments: bytes(commentsPtr, commentsLen).slice(),
            parse: () => parseAst(options, filePath, sourceText, lang, sourceType),
          },
          hasBom,
        );

        const json = lintFile(
          filePath,
          NO_BUFFER_ID,
          null,
          ruleIds,
          optionsIds,
          str(settingsPtr, settingsLen),
          str(globalsPtr, globalsLen),
          collectTimings,
          null,
        );
        return respond(0, json ?? "");
      },

      create_workspace(uriPtr: number, uriLen: number): number {
        createWorkspace(str(uriPtr, uriLen));
        return respond(0, "");
      },

      destroy_workspace(uriPtr: number, uriLen: number): number {
        destroyWorkspace(str(uriPtr, uriLen));
        return respond(0, "");
      },

      tsgolint_run(
        cwdPtr: number,
        cwdLen: number,
        argsPtr: number,
        argsLen: number,
        stdinPtr: number,
        stdinLen: number,
      ): number {
        if (options.runTsgolint === undefined) {
          return respond(-1, "The host does not run tsgolint");
        }
        try {
          const { exitCode, stdout } = options.runTsgolint(
            str(cwdPtr, cwdLen),
            str(argsPtr, argsLen).split("\0"),
            bytes(stdinPtr, stdinLen).slice(),
          );
          return respond(exitCode, stdout);
        } catch (err) {
          return respond(-1, getErrorMessage(err));
        }
      },

      response_len(): number {
        return response.length;
      },

      response_read(dstPtr: number): void {
        bytes(dstPtr, response.length).set(response);
        response = new Uint8Array();
      },
    },
  };
}

/**
 * Compile the options schemas of a plugin's rules, before the CLI loads the plugin.
 *
 * Loading a plugin compiles the schemas with AJV, which generates code.
 * Cloudflare Workers only allow generating code at startup, so a Worker calls this at startup
 * for each plugin it can load. Loading the plugin then reuses the compiled schemas.
 *
 * @param plugin - Plugin
 */
export function compilePluginSchemas(plugin: Plugin): void {
  for (const rule of Object.values(plugin.rules)) {
    const schema = rule.meta?.schema;
    if (schema != null && schema !== false) compileSchema(schema);
  }
}

// Prototype of AST nodes, with a lazy `loc` getter, as the raw transfer deserializer gives them
const NodeProto = Object.create(Object.prototype, {
  loc: {
    // Note: Not configurable
    get() {
      return getNodeLoc(this);
    },
    enumerable: true,
  },
});

/**
 * Parse the file with oxc-parser, and give the AST the shape the raw transfer deserializer gives it:
 * TS-ESTree, without `ParenthesizedExpression`s, with `range`, `loc`, and `parent` on every node,
 * and `comments` and `tokens` on the `Program`.
 */
function parseAst(
  options: OxlintHostOptions,
  filePath: string,
  sourceText: string,
  lang: string,
  sourceType: string,
): Program {
  const result = options.parseSync(filePath, sourceText, {
    lang,
    sourceType,
    astType: "ts",
    range: true,
    preserveParens: false,
  });
  const { node, fixes } = JSON.parse(result.program);

  // Set `value` of `Literal`s which are `BigInt`s or `RegExp`s, as oxc-parser does
  for (const fixPath of fixes) {
    let literal = node;
    for (const key of fixPath) literal = literal[key];
    if (literal.bigint) {
      literal.value = BigInt(literal.bigint);
    } else {
      try {
        literal.value = RegExp(literal.regex.pattern, literal.regex.flags);
      } catch {
        // Invalid regexp, or valid regexp using syntax not supported by this JS engine
      }
    }
  }

  // Hashbang is a `Shebang` comment instead
  const program = {
    __proto__: NodeProto,
    type: "Program",
    body: node.body,
    sourceType: node.sourceType,
    get comments() {
      if (comments === null) initComments();
      return comments;
    },
    get tokens() {
      if (tokens === null) initTokens();
      return tokens;
    },
    start: node.start,
    end: node.end,
    range: node.range,
    parent: null,
  };
  setParents(program.body, program);
  return program as unknown as Program;
}

/**
 * Set prototype and `parent` of AST nodes in `value`, and of their descendants.
 * @param value - Node, array of nodes, or other property value of a node
 * @param parent - Parent node
 */
function setParents(value: unknown, parent: object): void {
  if (typeof value !== "object" || value === null) return;
  if (Array.isArray(value)) {
    for (let i = 0; i < value.length; i++) setParents(value[i], parent);
    return;
  }

  const node = value as Record<string, unknown>;
  if (typeof node.type !== "string") return;

  Object.setPrototypeOf(node, NodeProto);
  const keys = Object.keys(node);
  node.parent = parent;
  for (let i = 0; i < keys.length; i++) setParents(node[keys[i]], node);
}
