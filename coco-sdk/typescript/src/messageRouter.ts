import { ProcessError, TransportClosedError } from "./errors.js";
import type { ClientRequest } from "./generated/protocol.js";
import type { JsonObject, Transport } from "./transport.js";

const JSONRPC_VERSION = "2.0";

export type ServerRequestHandler = (message: JsonObject, signal: AbortSignal) => Promise<boolean>;

type Pending = {
  resolve(value: JsonObject): void;
  reject(reason: unknown): void;
};

export class MessageRouter {
  private readonly pending = new Map<number | string, Pending>();
  private readonly ignoredResponses = new Set<number | string>();
  private readonly earlyResponses = new Map<number | string, JsonObject | Error>();
  private readonly events = new AsyncQueue<JsonObject>();
  private readonly serverRequestControllers = new Map<number | string, AbortController>();
  private closed = false;
  private reader?: Promise<void>;

  constructor(
    private readonly transport: Transport,
    private readonly serverRequestHandler?: ServerRequestHandler,
  ) {}

  start(): void {
    this.reader ??= this.readMessages();
  }

  async close(): Promise<void> {
    this.failAll(new TransportClosedError("transport closed"));
    await this.transport.close();
    await this.reader;
  }

  async request(request: ClientRequest): Promise<JsonObject> {
    const id = this.transport.nextRequestId();
    const early = this.earlyResponses.get(id);
    if (early) {
      this.earlyResponses.delete(id);
      await this.sendTypedRequest(id, request);
      if (early instanceof Error) throw early;
      return early;
    }
    if (this.closed) {
      throw new TransportClosedError("transport closed");
    }
    const response = new Promise<JsonObject>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
    try {
      const [, result] = await Promise.all([this.sendTypedRequest(id, request), response]);
      return result;
    } catch (error) {
      this.pending.delete(id);
      throw error;
    }
  }

  async notify(request: ClientRequest): Promise<void> {
    const id = this.transport.nextRequestId();
    this.ignoredResponses.add(id);
    try {
      await this.sendTypedRequest(id, request);
    } catch (error) {
      this.ignoredResponses.delete(id);
      throw error;
    }
  }

  async respond(id: number | string, result: unknown): Promise<void> {
    await this.transport.sendLine(
      JSON.stringify({ jsonrpc: JSONRPC_VERSION, id, result: result ?? {} }),
    );
  }

  async respondError(id: number | string, message: string, code = -32603): Promise<void> {
    await this.transport.sendLine(
      JSON.stringify({ jsonrpc: JSONRPC_VERSION, id, error: { code, message } }),
    );
  }

  async nextEvent(signal?: AbortSignal): Promise<JsonObject> {
    return this.events.shift(signal);
  }

  private async sendTypedRequest(id: number | string, request: ClientRequest): Promise<void> {
    const envelope: JsonObject = {
      jsonrpc: JSONRPC_VERSION,
      id,
      method: request.method,
    };
    if ("params" in request && request.params !== undefined) {
      envelope.params = request.params as unknown;
    }
    await this.transport.sendLine(JSON.stringify(envelope));
  }

  private async readMessages(): Promise<void> {
    try {
      for await (const message of this.transport.readLines()) {
        if (message.jsonrpc !== JSONRPC_VERSION) {
          throw new ProcessError(`invalid JSON-RPC version from coco: ${String(message.jsonrpc)}`);
        }
        if ("id" in message && "result" in message) {
          this.routeResponse(message);
        } else if ("error" in message) {
          this.routeError(message);
        } else if ("id" in message && "method" in message) {
          void this.routeServerRequest(message);
        } else if ("method" in message) {
          if (message.method === "control/cancelRequest") {
            const params = (message.params ?? {}) as JsonObject;
            const requestId = params.request_id as number | string | undefined;
            if (requestId !== undefined) {
              // Purge the correlation entry immediately — a handler that
              // never settles must not leak it until close.
              const controller = this.serverRequestControllers.get(requestId);
              this.serverRequestControllers.delete(requestId);
              controller?.abort();
            }
          } else {
            this.events.push(message);
          }
        } else {
          throw new ProcessError(`invalid JSON-RPC message from coco: ${JSON.stringify(message)}`);
        }
      }
      this.failAll(new TransportClosedError("transport closed"));
    } catch (error) {
      this.failAll(error instanceof Error ? error : new Error(String(error)));
    }
  }

  private routeResponse(message: JsonObject): void {
    const id = message.id as number | string | undefined;
    if (id === undefined) return;
    if (this.ignoredResponses.delete(id)) return;
    const pending = this.pending.get(id);
    const result = (message.result ?? {}) as JsonObject;
    if (pending) {
      this.pending.delete(id);
      pending.resolve(result);
    } else {
      this.earlyResponses.set(id, result);
    }
  }

  private routeError(message: JsonObject): void {
    const id = message.id as number | string | undefined;
    const errorObject = (message.error ?? {}) as { code?: number; message?: string };
    if (id !== undefined && this.ignoredResponses.delete(id)) return;
    const error = new ProcessError(`coco rejected request ${String(id)}: ${errorObject.message ?? ""}`, {
      exitCode: errorObject.code,
    });
    if (id === undefined) {
      this.events.pushError(error);
      return;
    }
    const pending = this.pending.get(id);
    if (pending) {
      this.pending.delete(id);
      pending.reject(error);
    } else {
      this.earlyResponses.set(id, error);
    }
  }

  private async routeServerRequest(message: JsonObject): Promise<void> {
    const id = message.id as number | string | undefined;
    if (id === undefined) return;
    const controller = new AbortController();
    this.serverRequestControllers.set(id, controller);
    try {
      const handled = this.serverRequestHandler
        ? await this.serverRequestHandler(message, controller.signal)
        : false;
      if (controller.signal.aborted) return;
      if (!handled) {
        await this.respondError(id, `unsupported server request: ${String(message.method ?? "")}`, -32601);
      }
    } catch (error) {
      // A handler that honors the AbortSignal by rejecting (e.g.
      // `signal.throwIfAborted()`) is benign on the losing side of a
      // broadcast — swallow it: no error reply, no event-loop error.
      if (controller.signal.aborted) return;
      try {
        await this.respondError(
          id,
          error instanceof Error ? error.message : String(error),
          -32603,
        );
      } catch (replyError) {
        this.events.pushError(replyError instanceof Error ? replyError : new Error(String(replyError)));
      }
    } finally {
      this.serverRequestControllers.delete(id);
    }
  }

  private failAll(error: Error): void {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
    for (const controller of this.serverRequestControllers.values()) controller.abort();
    this.serverRequestControllers.clear();
    this.events.pushError(error);
  }
}

class AsyncQueue<T> {
  private values: Array<T | Error> = [];
  private waiters: Array<(value: T | Error) => void> = [];

  push(value: T): void {
    const waiter = this.waiters.shift();
    if (waiter) waiter(value);
    else this.values.push(value);
  }

  pushError(error: Error): void {
    const waiter = this.waiters.shift();
    if (waiter) waiter(error);
    else this.values.push(error);
  }

  async shift(signal?: AbortSignal): Promise<T> {
    const value = this.values.shift();
    if (value !== undefined) {
      if (value instanceof Error) throw value;
      return value;
    }
    if (signal?.aborted) {
      throw abortReason(signal);
    }
    const resolved = await new Promise<T | Error>((resolve, reject) => {
      const waiter = (item: T | Error) => {
        cleanup();
        resolve(item);
      };
      const onAbort = () => {
        cleanup();
        reject(abortReason(signal));
      };
      const cleanup = () => {
        const index = this.waiters.indexOf(waiter);
        if (index !== -1) this.waiters.splice(index, 1);
        signal?.removeEventListener("abort", onAbort);
      };
      this.waiters.push(waiter);
      signal?.addEventListener("abort", onAbort, { once: true });
    });
    if (resolved instanceof Error) throw resolved;
    return resolved;
  }
}

function abortReason(signal?: AbortSignal): Error {
  return signal?.reason instanceof Error ? signal.reason : new Error("Operation aborted");
}
