export { CocoClient } from "./client.js";
export type {
  CanUseTool,
  CocoClientOptions,
  HookHandler,
  McpMessageHandler,
  TurnOptions,
} from "./client.js";
export { query } from "./query.js";
export type { QueryOptions } from "./query.js";
export {
  CLINotFoundError,
  CLIConnectionError,
  CocoSDKError,
  JSONDecodeError,
  ProcessError,
  SessionNotFoundError,
  TransportClosedError,
} from "./errors.js";
export { findCocoBinary, resolveCocoRuntime } from "./runtime.js";
export type { CocoRuntime } from "./runtime.js";
export { DEEPSEEK, ModelAlias, modelSpecToCliArg, thinking } from "./types.js";
export type { ModelSpec } from "./types.js";
export * from "./generated/protocol.js";
