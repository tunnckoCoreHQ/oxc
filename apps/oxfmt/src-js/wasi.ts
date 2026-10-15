/*
 * Host side of the oxfmt CLI built for `wasm32-wasip1`: the `oxfmt_host` imports.
 *
 * They stand in for the JS callbacks which the NAPI build gets from `cli.ts`,
 * and take the same callbacks, with the same contracts.
 * `toFormatFileResult` and `toNullable` adapt the `cli-worker.ts` functions to these contracts.
 *
 * A call passes strings as pointer and length, and returns a status (`0` for success).
 * The host keeps the response bytes until `response_read` copies them into wasm memory.
 * With the `jspi` option, callbacks can be asynchronous (JSPI), and the host calls
 * the command's `_start` through `WebAssembly.promising`.
 */

import type { FormatFileResult } from "./libs/napi-callbacks";

export { toFormatFileResult, toNullable } from "./libs/napi-callbacks";

type MaybePromise<T> = T | Promise<T>;

export interface OxfmtHostOptions {
  /**
   * Evaluate a JS/TS config file. Same contract as `loadJsConfig` (or `loadVitePlusConfig`) of `cli/js_config.ts`:
   * the config object, or `null` to skip the file.
   */
  loadJsConfig: (path: string) => MaybePromise<object | null>;
  /**
   * Format a whole file with Prettier. `options` includes `parser` and `filepath`.
   */
  formatFile: (options: Record<string, any>, code: string) => MaybePromise<FormatFileResult>;
  /**
   * Format embedded code with Prettier. `options` includes `parser`. `null` on error.
   */
  formatEmbeddedCode: (options: Record<string, any>, code: string) => MaybePromise<string | null>;
  /**
   * Format embedded code into a Prettier `Doc` JSON string. `options` includes `parser`. `null` on error.
   */
  formatEmbeddedDoc: (options: Record<string, any>, code: string) => MaybePromise<string | null>;
  /**
   * Sort Tailwind CSS classes. `options` includes `filepath`. `null` on error.
   */
  sortTailwindClasses: (
    options: Record<string, any>,
    classes: string[],
  ) => MaybePromise<string[] | null>;
  /**
   * `true` if the host calls the command's `_start` through `WebAssembly.promising` (JSPI).
   * Then callbacks may return promises.
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
 * Create the `oxfmt_host` imports.
 * @param options - Host callbacks
 * @param getMemory - Get memory of the wasm instance
 * @returns Imports object, to merge into the instance's imports
 */
export function createOxfmtHostImports(
  options: OxfmtHostOptions,
  getMemory: () => { buffer: ArrayBuffer },
): { oxfmt_host: Record<string, Function> } {
  let response: Uint8Array = new Uint8Array();

  const respond = (status: number, body: string): number => {
    response = textEncoder.encode(body);
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

  // Pass the result of `run` to `onValue`, after waiting for it if it is a promise.
  // `onError` handles a throw or a rejection.
  const settle = <T>(
    run: () => MaybePromise<T>,
    onValue: (value: T) => number,
    onError: (err: unknown) => number,
  ): number | Promise<number> => {
    let value;
    try {
      value = run();
    } catch (err) {
      return onError(err);
    }
    return value instanceof Promise ? value.then(onValue, onError) : onValue(value);
  };

  // `null` from embedded formatting or Tailwind sorting is a non-zero status, Rust keeps the original
  const nullable = (value: string | null) => (value === null ? respond(1, "") : respond(0, value));
  const fail = () => respond(1, "");

  return {
    oxfmt_host: {
      load_js_config: suspending((pathPtr, pathLen) =>
        settle(
          () => options.loadJsConfig(str(pathPtr, pathLen)),
          (config) => respond(0, JSON.stringify(config) ?? "null"),
          (err) => respond(1, err instanceof Error ? err.message : String(err)),
        ),
      ),

      format_file: suspending((optionsPtr, optionsLen, codePtr, codeLen) =>
        settle(
          () => options.formatFile(JSON.parse(str(optionsPtr, optionsLen)), str(codePtr, codeLen)),
          (result) => (result.ok ? respond(0, result.code) : respond(1, result.error)),
          (err) => respond(1, String(err)),
        ),
      ),

      format_embedded: suspending((optionsPtr, optionsLen, codePtr, codeLen) =>
        settle(
          () =>
            options.formatEmbeddedCode(
              JSON.parse(str(optionsPtr, optionsLen)),
              str(codePtr, codeLen),
            ),
          nullable,
          fail,
        ),
      ),

      format_embedded_doc: suspending((optionsPtr, optionsLen, codePtr, codeLen) =>
        settle(
          () =>
            options.formatEmbeddedDoc(
              JSON.parse(str(optionsPtr, optionsLen)),
              str(codePtr, codeLen),
            ),
          nullable,
          fail,
        ),
      ),

      sort_tailwind_classes: suspending((optionsPtr, optionsLen, classesPtr, classesLen) =>
        settle(
          () =>
            options.sortTailwindClasses(
              JSON.parse(str(optionsPtr, optionsLen)),
              JSON.parse(str(classesPtr, classesLen)),
            ),
          (sorted) => nullable(sorted === null ? null : JSON.stringify(sorted)),
          fail,
        ),
      ),

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
