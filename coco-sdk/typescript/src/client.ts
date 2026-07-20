import {
  ClientRequestMethod,
  NotificationMethod,
  ServerRequestMethod,
  type ApprovalDecision,
  type ClientRequest,
  type HookCallbackParams,
  type HookCallbackOutput,
  type InitializeParams,
  type McpRouteMessageParams,
  type PermissionMode,
  type SessionTarget,
  type SessionStartResult,
  type ServerNotification,
  type ThinkingLevel,
  type TurnEndedParams,
} from "./generated/protocol.js";
import { MessageRouter } from "./messageRouter.js";
import { ProcessError } from "./errors.js";
import { modelSpecToCliArg, type ModelSpec } from "./types.js";
import { SubprocessCLITransport, type JsonObject, type Transport } from "./transport.js";

export type CanUseTool = (
  toolName: string,
  input: Record<string, unknown>,
  signal?: AbortSignal,
) => Promise<ApprovalDecision>;
export type HookHandler = (
  callbackId: string,
  eventType: string,
  input: unknown,
) => Promise<HookCallbackOutput | Record<string, unknown> | null | undefined>;
export type McpMessageHandler = (serverName: string, message: unknown) => Promise<unknown>;

export type TurnOptions = {
  signal?: AbortSignal;
};

export type CocoClientOptions = {
  prompt: string;
  modelsMain?: string | ModelSpec | null;
  maxTurns?: number | null;
  maxBudgetUsd?: number | null;
  cwd?: string | null;
  permissionMode?: PermissionMode | null;
  systemPrompt?: string | null;
  appendSystemPrompt?: string | null;
  agents?: InitializeParams["agents"];
  hooks?: InitializeParams["hooks"];
  sdkMcpServers?: string[] | null;
  jsonSchema?: Record<string, unknown> | null;
  agentProgressSummaries?: boolean | null;
  promptSuggestions?: boolean | null;
  canUseTool?: CanUseTool | null;
  hookHandlers?: Record<string, HookHandler>;
  mcpMessageHandlers?: Record<string, McpMessageHandler>;
  env?: Record<string, string>;
  binaryPath?: string | null;
  transport?: Transport;
  signal?: AbortSignal;
};

const TURN_START_BUSY_MARKER = "already running";
const TURN_START_RETRY_INITIAL_MS = 10;
const TURN_START_RETRY_MAX_DELAY_MS = 200;
const TURN_START_RETRY_TIMEOUT_MS = 5000;

export class CocoClient {
  private readonly initialPrompt: string;
  private readonly modelsMain?: string;
  private readonly options: CocoClientOptions;
  private readonly transport: Transport;
  private readonly canUseTool?: CanUseTool | null;
  private readonly hookHandlers = new Map<string, HookHandler>();
  private readonly mcpMessageHandlers = new Map<string, McpMessageHandler>();
  private router: MessageRouter | null = null;
  private started = false;
  private sessionId: string | null = null;

  constructor(options: CocoClientOptions) {
    this.initialPrompt = options.prompt;
    this.modelsMain = options.modelsMain ? modelSpecToCliArg(options.modelsMain) : undefined;
    this.options = options;
    this.canUseTool = options.canUseTool;
    for (const [callbackId, handler] of Object.entries(options.hookHandlers ?? {})) {
      this.hookHandlers.set(callbackId, handler);
    }
    for (const [serverName, handler] of Object.entries(options.mcpMessageHandlers ?? {})) {
      this.mcpMessageHandlers.set(serverName, handler);
    }

    const cliArgs: string[] = [];
    if (this.modelsMain) cliArgs.push("--models.main", this.modelsMain);
    this.transport =
      options.transport ??
      new SubprocessCLITransport({
        binaryPath: options.binaryPath,
        cwd: options.cwd,
        env: options.env,
        cliArgs,
        signal: options.signal,
      });
  }

  async start(): Promise<void> {
    try {
      throwIfAborted(this.options.signal);
      await this.transport.start();
      this.router = new MessageRouter(this.transport, (message, signal) =>
        this.handleServerRequest(message, signal),
      );
      this.router.start();
      await this.sendInitialize();
      await this.sendSessionStart();
      await this.sendTurnStart(this.initialPrompt);
      this.started = true;
    } catch (error) {
      await this.close();
      throw error;
    }
  }

  async *events(options: TurnOptions = {}): AsyncGenerator<ServerNotification> {
    const router = this.requireRouter();
    while (true) {
      const event = (await router.nextEvent(options.signal)) as unknown as ServerNotification;
      yield event;
      if (event.method === NotificationMethod.TURN_ENDED) break;
    }
  }

  async *send(text: string, options: TurnOptions = {}): AsyncGenerator<ServerNotification> {
    throwIfAborted(options.signal);
    await this.startTurnWithRetry(
      {
        method: ClientRequestMethod.TURN_START,
        params: {
          target: this.sessionTarget(),
          prompt: text,
          composer: { next_attachment_label: 0 },
        },
      },
      options.signal,
    );
    try {
      yield* this.events(options);
    } catch (error) {
      if (options.signal?.aborted) {
        await this.interrupt().catch(() => {});
      }
      throw error;
    }
  }

  async *streamText(options: TurnOptions = {}): AsyncGenerator<string> {
    for await (const event of this.events(options)) {
      if (event.method === NotificationMethod.AGENT_MESSAGE_DELTA) {
        yield event.params.delta;
      }
    }
  }

  async waitForTurnEnded(options: TurnOptions = {}): Promise<TurnEndedParams | null> {
    for await (const event of this.events(options)) {
      if (event.method === NotificationMethod.TURN_ENDED) return event.params;
    }
    return null;
  }

  async getFinalText(options: TurnOptions = {}): Promise<string> {
    const parts: string[] = [];
    for await (const delta of this.streamText(options)) parts.push(delta);
    return parts.join("");
  }

  async interrupt(): Promise<void> {
    await this.notify({
      method: ClientRequestMethod.TURN_INTERRUPT,
      params: this.sessionTarget(),
    });
  }

  async setPermissionMode(mode: PermissionMode): Promise<void> {
    await this.notify({
      method: ClientRequestMethod.CONTROL_SET_PERMISSION_MODE,
      params: { target: this.sessionTarget(), mode },
    });
  }

  async setThinking(level: ThinkingLevel | null): Promise<void> {
    await this.notify({
      method: ClientRequestMethod.CONTROL_SET_THINKING,
      params: { target: this.sessionTarget(), thinking_level: level },
    });
  }

  async closeSession(sessionId: string | null = null): Promise<void> {
    const targetSessionId = sessionId ?? this.sessionId;
    if (!targetSessionId) {
      throw new Error("no session id to close");
    }
    await this.request({
      method: ClientRequestMethod.SESSION_CLOSE,
      params: { target: this.closeTarget(targetSessionId) },
    });
    if (targetSessionId === this.sessionId) {
      this.sessionId = null;
    }
  }

  async deleteSession(sessionId: string): Promise<void> {
    await this.request({
      method: ClientRequestMethod.SESSION_DELETE,
      params: { target: { session_id: sessionId } },
    });
  }

  onHook(callbackId: string, handler: HookHandler): void {
    this.hookHandlers.set(callbackId, handler);
  }

  onMcpMessage(serverName: string, handler: McpMessageHandler): void {
    this.mcpMessageHandlers.set(serverName, handler);
  }

  async close(): Promise<void> {
    if (this.router) {
      await this.router.close();
      this.router = null;
    } else {
      await this.transport.close();
    }
    this.started = false;
    this.sessionId = null;
  }

  private async sendInitialize(): Promise<void> {
    await this.request({
      method: ClientRequestMethod.INITIALIZE,
      params: {
        agents: this.options.agents ?? null,
        hooks: this.options.hooks ?? null,
        client_mcp_servers: this.options.sdkMcpServers ?? null,
        agentProgressSummaries: this.options.agentProgressSummaries ?? null,
        prompt_suggestions: this.options.promptSuggestions ?? null,
      },
    });
  }

  private async sendSessionStart(): Promise<void> {
    const result = (await this.request({
      method: ClientRequestMethod.SESSION_START,
      params: {
        model: this.modelsMain ?? null,
        max_turns: this.options.maxTurns ?? null,
        max_budget_usd: this.options.maxBudgetUsd ?? null,
        cwd: this.options.cwd ?? null,
        permission_mode: this.options.permissionMode ?? null,
        system_prompt: this.options.systemPrompt ?? null,
        append_system_prompt: this.options.appendSystemPrompt ?? null,
        json_schema: this.options.jsonSchema ?? null,
      },
    })) as unknown as SessionStartResult;
    this.sessionId = result.session_id;
  }

  private async sendTurnStart(prompt: string): Promise<void> {
    await this.request({
      method: ClientRequestMethod.TURN_START,
      params: {
        target: this.sessionTarget(),
        prompt,
        composer: { next_attachment_label: 0 },
      },
    });
  }

  private sessionTarget(): SessionTarget {
    if (!this.sessionId) {
      throw new Error("no active session target");
    }
    return { session_id: this.sessionId };
  }

  private closeTarget(sessionId: string): SessionTarget {
    return { session_id: sessionId };
  }

  private async startTurnWithRetry(request: ClientRequest, signal?: AbortSignal): Promise<void> {
    const deadline = Date.now() + TURN_START_RETRY_TIMEOUT_MS;
    let delay = TURN_START_RETRY_INITIAL_MS;
    while (true) {
      try {
        throwIfAborted(signal);
        await this.request(request);
        return;
      } catch (error) {
        if (
          !(error instanceof ProcessError) ||
          !error.message.includes(TURN_START_BUSY_MARKER) ||
          Date.now() >= deadline
        ) {
          throw error;
        }
        await sleep(Math.min(delay, Math.max(0, deadline - Date.now())), signal);
        delay = Math.min(delay * 2, TURN_START_RETRY_MAX_DELAY_MS);
      }
    }
  }

  private async request(request: ClientRequest): Promise<JsonObject> {
    return this.requireRouter().request(request);
  }

  private async notify(request: ClientRequest): Promise<void> {
    return this.requireRouter().notify(request);
  }

  private requireRouter(): MessageRouter {
    if (!this.router) {
      this.router = new MessageRouter(this.transport, (message, signal) =>
        this.handleServerRequest(message, signal),
      );
      this.router.start();
    }
    return this.router;
  }

  private async handleServerRequest(message: JsonObject, signal: AbortSignal): Promise<boolean> {
    const method = message.method;
    const id = message.id as number | string | undefined;
    const params = (message.params ?? {}) as Record<string, unknown>;
    if (id === undefined) return false;

    if (method === ServerRequestMethod.APPROVAL_ASK_FOR_APPROVAL) {
      if (!this.canUseTool) {
        // No permission callback configured: reply with a JSON-RPC error,
        // NOT a `deny`. Approvals are broadcast to all Full connections and
        // the first VALID reply wins — a deny here would consume the
        // broadcast and cancel a human peer's prompt in multi-client
        // sessions. An error reply only withdraws this client. Single-client
        // headless keeps the same observable outcome: sole recipient errors
        // → the server cancels the request → the tool is denied.
        await this.requireRouter().respondError(id, "no approval handler configured", -32601);
        return true;
      }
      // Pass the broadcast-cancellation signal so a losing client can
      // dismiss an in-flight prompt.
      const decision = await this.canUseTool(
        String(params.tool_name ?? ""),
        asRecord(params.input),
        signal,
      );
      if (signal.aborted) return true;
      await this.requireRouter().respond(id, {
        request_id: params.request_id ?? "",
        decision,
      });
      return true;
    }

    if (method === ServerRequestMethod.HOOK_CALLBACK) {
      const hookParams = params as unknown as HookCallbackParams;
      const handler = this.hookHandlers.get(hookParams.callback_id);
      const output = handler
        ? normalizeHookOutput(
            await handler(hookParams.callback_id, hookParams.event_type, hookParams.input),
          )
        : {};
      if (signal.aborted) return true;
      await this.requireRouter().respond(id, { output });
      return true;
    }

    if (method === ServerRequestMethod.MCP_ROUTE_MESSAGE) {
      const routeParams = params as unknown as McpRouteMessageParams;
      const handler = this.mcpMessageHandlers.get(routeParams.server_name);
      const response = handler
        ? await handler(routeParams.server_name, routeParams.message)
        : unsupportedMcpResponse(routeParams.message, routeParams.server_name);
      if (signal.aborted) return true;
      await this.requireRouter().respond(id, { message: response });
      return true;
    }

    return false;
  }
}

function normalizeHookOutput(output: HookCallbackOutput | Record<string, unknown> | null | undefined): Record<string, unknown> {
  if (!output || typeof output !== "object" || Array.isArray(output)) return {};
  return output as Record<string, unknown>;
}

function unsupportedMcpResponse(message: unknown, serverName: string): Record<string, unknown> {
  const msg = asRecord(message);
  return {
    jsonrpc: "2.0",
    id: "id" in msg ? msg.id : null,
    error: {
      code: -32601,
      message: `SDK MCP server is not registered: ${serverName}`,
    },
  };
}

function asRecord(value: unknown): Record<string, unknown> {
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return {};
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) {
    throw signal.reason instanceof Error ? signal.reason : new Error("Operation aborted");
  }
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      clearTimeout(timeout);
      reject(signal?.reason instanceof Error ? signal.reason : new Error("Operation aborted"));
    };
    const timeout = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}
